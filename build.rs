use std::{
    borrow::Cow,
    collections::BTreeMap,
    env::var_os,
    fs::{canonicalize, copy, create_dir_all, read, read_dir, remove_dir_all, write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use pnpm_config::Config;
use pnpm_lockfile::{LazyLockfile, Lockfile, MaybeLazyLockfile};
use pnpm_network::ThrottledClient;
use pnpm_package_manager::{Install, ProjectMutation, ResolvedPackages, UpdateSeedPolicy};
use pnpm_package_manifest::{DependencyGroup, PackageManifest};
use pnpm_reporter::SilentReporter;
use pnpm_tarball::MemCache;
use rolldown::{
    Bundler, BundlerOptions, BundlerTransformOptions, CodeSplittingMode, Either, InputItem,
    OutputFormat, Platform, ResolveOptions,
    plugin::{
        HookResolveIdArgs, HookResolveIdOutput, HookResolveIdReturn, HookTransformArgs,
        HookTransformOutput, HookTransformOutputMap, HookTransformReturn, HookUsage, Plugin,
        PluginContext, SharedTransformPluginContext,
    },
};
use rolldown_common::Output;
use semver::Version;
use serde_json::{Value, from_slice, to_string};
use tokio::runtime::Builder;

const PNPM_SOURCE: &str = "pnpm/pnpm@87b9504492d879c05c550def1c8058214ebaac83";
const REGISTRY_SPECIFIER: &str = "textlint-v8:registry";

#[derive(Debug)]
struct RulePackage {
    id: String,
    package: String,
    preset_prefix: Option<String>,
}

#[derive(Debug)]
struct TextlintBundlePlugin {
    dictionary_loader: String,
    dictionary_base_loader: String,
    conjunctive_particle_rule: String,
}

struct BundleOutput {
    bytes: Vec<u8>,
    module_ids: Vec<PathBuf>,
}

impl Plugin for TextlintBundlePlugin {
    fn name(&self) -> Cow<'static, str> {
        "textlint-v8-build".into()
    }

    async fn resolve_id(
        &self,
        _ctx: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> HookResolveIdReturn {
        if (args.specifier.ends_with("NodeDictionaryLoader")
            || args.specifier.ends_with("NodeDictionaryLoader.js"))
            && args
                .importer
                .is_some_and(|importer| importer.contains("/kuromoji/"))
        {
            return Ok(Some(HookResolveIdOutput::from_id(
                self.dictionary_loader.clone(),
            )));
        }
        if args.specifier == "kuromoji/src/loader/DictionaryLoader.js"
            && args.importer == Some(self.dictionary_loader.as_str())
        {
            return Ok(Some(HookResolveIdOutput::from_id(
                self.dictionary_base_loader.clone(),
            )));
        }
        Ok(None)
    }

    async fn transform(
        &self,
        _ctx: SharedTransformPluginContext,
        args: &HookTransformArgs<'_>,
    ) -> HookTransformReturn {
        if args.id != self.conjunctive_particle_rule {
            return Ok(None);
        }

        let mut source = args.code.to_string();
        for (before, after) in [
            (
                "return (0, _kuromojin.getTokenizer)().then(tokenizer => {\n        var checkSentence = sentence => {",
                "var checkSentence = async sentence => {",
            ),
            (
                "var tokens = tokenizer.tokenizeForSentence(sentenceText);",
                "var tokens = await (0, _kuromojin.tokenize)(sentenceText);",
            ),
            (
                "        sentences.forEach(checkSentence);\n      });",
                "        return Promise.all(sentences.map(checkSentence));",
            ),
        ] {
            if source.matches(before).count() != 1 {
                bail!(
                    "textlint-rule-no-doubled-conjunctive-particle-ga changed; \
                     expected cache patch source exactly once"
                );
            }
            source = source.replacen(before, after, 1);
        }

        Ok(Some(HookTransformOutput {
            code: Some(source),
            map: HookTransformOutputMap::Null,
            ..Default::default()
        }))
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ResolveId | HookUsage::Transform
    }
}

fn package_for_rule_id(rule_id: &str) -> Result<(String, Option<String>)> {
    let (scope, name) = if let Some(scoped) = rule_id.strip_prefix('@') {
        let (scope, name) = scoped
            .split_once('/')
            .with_context(|| format!("scoped textlint rule ID must contain '/': {rule_id}"))?;
        if scope.is_empty() || name.is_empty() || name.contains('/') {
            bail!("invalid scoped textlint rule ID: {rule_id}");
        }
        (Some(format!("@{scope}")), name)
    } else {
        if rule_id.is_empty() || rule_id.contains('/') {
            bail!("invalid unscoped textlint rule ID: {rule_id}");
        }
        (None, rule_id)
    };

    let package = match &scope {
        Some(scope) => format!("{scope}/textlint-rule-{name}"),
        None => format!("textlint-rule-{name}"),
    };
    let preset_prefix = name.strip_prefix("preset-").map(|name| match &scope {
        Some(scope) => format!("{scope}/{name}"),
        None => name.to_owned(),
    });
    if preset_prefix.as_deref().is_some_and(str::is_empty) {
        bail!("preset textlint rule ID must have a name after 'preset-': {rule_id}");
    }
    Ok((package, preset_prefix))
}

fn rule_packages(config: &Value, manifest: &Value) -> Result<Vec<RulePackage>> {
    let config = config
        .as_object()
        .context("textlint config must be an object")?;
    let rules = config
        .get("rules")
        .and_then(Value::as_object)
        .context("textlint config must contain a rules object")?;
    let dependencies = manifest
        .as_object()
        .and_then(|manifest| manifest.get("dependencies"))
        .and_then(Value::as_object)
        .context("package.json must contain a dependencies object")?;
    let mut packages = Vec::with_capacity(rules.len());

    for (rule_id, options) in rules {
        if !options.is_boolean() && !options.is_object() {
            bail!("configuration for {rule_id} must be a boolean or object");
        }
        let (package, preset_prefix) = package_for_rule_id(rule_id)?;
        let version = dependencies
            .get(&package)
            .and_then(Value::as_str)
            .with_context(|| {
                format!("textlint rule {rule_id} requires package.json dependency {package}")
            })?;
        Version::parse(version).with_context(|| {
            format!(
                "textlint rule dependency {package} must use a fixed semantic version, got {version}"
            )
        })?;
        packages.push(RulePackage {
            id: rule_id.clone(),
            package,
            preset_prefix,
        });
    }
    Ok(packages)
}

fn generate_registry(packages: &[RulePackage], out_dir: &Path) -> Result<PathBuf> {
    let mut source = String::new();
    for (index, rule) in packages.iter().enumerate() {
        source.push_str(&format!(
            "import rule{index} from {};\n",
            to_string(&rule.package).context("serialize rule package name")?
        ));
    }
    source.push_str("\nexport const standaloneRules = [\n");
    for (index, rule) in packages.iter().enumerate() {
        if rule.preset_prefix.is_none() {
            source.push_str(&format!(
                "  {{ ruleId: {}, rule: rule{index} }},\n",
                to_string(&rule.id).context("serialize rule ID")?
            ));
        }
    }
    source.push_str("] as const;\n\nexport const presetDefinitions = [\n");
    for (index, rule) in packages.iter().enumerate() {
        if let Some(prefix) = &rule.preset_prefix {
            source.push_str(&format!(
                "  {{ presetId: {}, ruleIdPrefix: {}, preset: rule{index} }},\n",
                to_string(&rule.id).context("serialize preset ID")?,
                to_string(prefix).context("serialize preset rule prefix")?
            ));
        }
    }
    source.push_str("] as const;\n");

    let path = out_dir.join("textlint-rule-registry.ts");
    write(&path, source).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

async fn install_dependencies(root: &Path, update_lockfile: bool) -> Result<()> {
    let manifest = PackageManifest::from_path(root.join("package.json"))
        .context("read package.json for the embedded pnpm installer")?;
    let modules_dir = root.join("node_modules");
    let mut config = Config::new();
    config.store_dir = root.join("pnpm-store").into();
    config.modules_dir = modules_dir.clone();
    config.virtual_store_dir = modules_dir.join(".pnpm");
    config.workspace_dir = Some(root.to_path_buf());
    config.lockfile = true;
    config.ignore_scripts = true;
    let config = config.leak();
    let lockfile = LazyLockfile::deferred(root.to_path_buf());
    let lockfile_path = root.join(Lockfile::FILE_NAME);
    let http_client = Arc::new(ThrottledClient::default());
    let resolved_packages = ResolvedPackages::new();

    Install {
        tarball_mem_cache: Arc::new(MemCache::new()),
        resolved_packages: &resolved_packages,
        http_client: &http_client,
        http_client_arc: Arc::clone(&http_client),
        config,
        manifest: &manifest,
        emit_initial_manifest: true,
        lockfile: MaybeLazyLockfile::Lazy(&lockfile),
        lockfile_path: Some(&lockfile_path),
        dependency_groups: [
            DependencyGroup::Prod,
            DependencyGroup::Dev,
            DependencyGroup::Optional,
        ],
        frozen_lockfile: !update_lockfile,
        prefer_frozen_lockfile: None,
        ignore_manifest_check: false,
        skip_runtimes: false,
        trust_lockfile: false,
        update_checksums: false,
        mutation: ProjectMutation::InstallWorkspace,
        installs_only: true,
        supported_architectures: None,
        node_linker: config.node_linker,
        lockfile_only: false,
        dry_run: false,
        persist_policy_excludes: false,
        update_seed_policy: UpdateSeedPolicy::KeepAll,
        preferred_versions_override: None,
        auth_override: None,
        resolution_observer: None,
        peer_issues_sink: None,
        deps_requiring_build_sink: None,
        catalogs_override: None,
        disable_optimistic_repeat_install: false,
        pnpmfile_hook_override: None,
        workspace_projects_override: None,
    }
    .run::<SilentReporter>()
    .await
    .context("install npm dependencies through the pnpm Rust library")?;

    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    create_dir_all(destination).with_context(|| format!("create {}", destination.display()))?;
    for entry in read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", source.display()))?;
        let source = entry.path();
        let destination = destination.join(entry.file_name());
        if entry
            .file_type()
            .with_context(|| format!("read file type for {}", source.display()))?
            .is_dir()
        {
            copy_tree(&source, &destination)?;
        } else {
            copy(&source, &destination).with_context(|| {
                format!("copy {} to {}", source.display(), destination.display())
            })?;
        }
    }
    Ok(())
}

fn prepare_workspace(source_root: &Path, out_dir: &Path) -> Result<PathBuf> {
    let workspace = out_dir.join("npm-workspace");
    if workspace.exists() {
        remove_dir_all(&workspace).with_context(|| format!("remove {}", workspace.display()))?;
    }
    create_dir_all(&workspace).with_context(|| format!("create {}", workspace.display()))?;
    for filename in ["package.json", "pnpm-lock.yaml"] {
        copy(source_root.join(filename), workspace.join(filename))
            .with_context(|| format!("copy {filename} into the build workspace"))?;
    }
    copy_tree(&source_root.join("js"), &workspace.join("js"))?;
    Ok(workspace)
}

fn find_package_manifests(directory: &Path, manifests: &mut Vec<PathBuf>) -> Result<()> {
    for entry in read_dir(directory).with_context(|| format!("read {}", directory.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", directory.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type for {}", path.display()))?;
        if file_type.is_dir() {
            find_package_manifests(&path, manifests)?;
        } else if entry.file_name() == "package.json" {
            manifests.push(path);
        }
    }
    Ok(())
}

fn append_rust_notices(notices: &mut String, source_root: &Path) -> Result<()> {
    let cargo_home = var_os("CARGO_HOME").map_or_else(
        || {
            var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".cargo"))
        },
        |path| Some(PathBuf::from(path)),
    );
    let cargo_home = cargo_home.context("neither CARGO_HOME nor HOME is set")?;
    let registry_sources = cargo_home.join("registry/src");
    let inventory = String::from_utf8(read(source_root.join("resources/RUNTIME_RUST_CRATES.txt"))?)
        .context("runtime Rust crate inventory is not UTF-8")?;
    for line in inventory.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, version) = line
            .split_once(' ')
            .with_context(|| format!("invalid runtime Rust crate inventory line: {line}"))?;
        let directory_name = format!("{name}-{version}");
        let mut package_dir = None;
        for registry in read_dir(&registry_sources).with_context(|| {
            format!("read Cargo registry sources {}", registry_sources.display())
        })? {
            let candidate = registry?.path().join(&directory_name);
            if candidate.is_dir() {
                package_dir = Some(candidate);
                break;
            }
        }
        let package_dir = package_dir.with_context(|| {
            format!("runtime Rust crate source is unavailable: {name} {version}")
        })?;
        let mut license_files = Vec::new();
        for entry in read_dir(&package_dir)
            .with_context(|| format!("read Rust crate {}", package_dir.display()))?
        {
            let entry = entry.context("read Rust crate entry")?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let filename = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if filename.starts_with("license")
                || filename.starts_with("licence")
                || filename.starts_with("copying")
                || filename.starts_with("notice")
            {
                license_files.push(entry.path());
            }
        }
        license_files.sort();
        if license_files.is_empty() {
            if name == "v8" {
                continue;
            }
            bail!("runtime Rust crate has no license file: {name} {version}");
        }
        notices.push_str(&format!("\n--- {name} {version} ---\n"));
        for path in license_files {
            notices.push_str(&format!(
                "\n[{}]\n",
                path.file_name().unwrap().to_string_lossy()
            ));
            notices.push_str(
                &String::from_utf8(read(&path)?).with_context(|| {
                    format!("Rust license file is not UTF-8: {}", path.display())
                })?,
            );
            if !notices.ends_with('\n') {
                notices.push('\n');
            }
        }
    }
    Ok(())
}

fn generate_notices(
    root: &Path,
    source_root: &Path,
    out_dir: &Path,
    module_ids: &[PathBuf],
) -> Result<()> {
    let mut manifests = Vec::new();
    find_package_manifests(&root.join("node_modules/.pnpm"), &mut manifests)?;
    let mut packages = BTreeMap::new();
    for manifest_path in manifests {
        let manifest: Value = from_slice(
            &read(&manifest_path)
                .with_context(|| format!("read npm manifest {}", manifest_path.display()))?,
        )
        .with_context(|| format!("parse npm manifest {}", manifest_path.display()))?;
        let Some(name) = manifest.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(version) = manifest.get("version").and_then(Value::as_str) else {
            continue;
        };
        let Some(license) = manifest.get("license").and_then(Value::as_str).or_else(|| {
            manifest
                .get("licenses")
                .and_then(Value::as_array)
                .and_then(|licenses| licenses.first())
                .and_then(Value::as_object)
                .and_then(|license| license.get("type"))
                .and_then(Value::as_str)
        }) else {
            continue;
        };
        let package_dir = manifest_path
            .parent()
            .context("npm manifest has no parent directory")?;
        let canonical_package_dir = canonicalize(package_dir)
            .with_context(|| format!("canonicalize npm package {}", package_dir.display()))?;
        if !module_ids
            .iter()
            .any(|module_id| module_id.starts_with(&canonical_package_dir))
        {
            continue;
        }
        let author = manifest.get("author").and_then(|author| {
            author.as_str().or_else(|| {
                author
                    .as_object()
                    .and_then(|author| author.get("name"))
                    .and_then(Value::as_str)
            })
        });
        let repository = manifest.get("repository").and_then(|repository| {
            repository.as_str().or_else(|| {
                repository
                    .as_object()
                    .and_then(|repository| repository.get("url"))
                    .and_then(Value::as_str)
            })
        });
        let mut attribution = String::new();
        if let Some(author) = author {
            attribution.push_str(&format!("Author: {author}\n"));
        }
        if let Some(repository) = repository {
            attribution.push_str(&format!("Source: {repository}\n"));
        }
        packages
            .entry((name.to_owned(), version.to_owned()))
            .or_insert((license.to_owned(), package_dir.to_path_buf(), attribution));
    }
    for module_id in module_ids {
        if module_id
            .components()
            .any(|component| component.as_os_str() == ".pnpm")
            && !packages.values().any(|(_, package_dir, _)| {
                module_id.starts_with(package_dir)
                    || canonicalize(package_dir)
                        .is_ok_and(|package_dir| module_id.starts_with(package_dir))
            })
        {
            bail!(
                "bundled npm module has no package license metadata: {}",
                module_id.display()
            );
        }
    }

    let mut notices = String::from(
        "THIRD-PARTY LICENSES AND NOTICES\n\nThis file is generated at build time from the locked dependencies and tracked attribution sources.\n\n",
    );
    notices.push_str("RUST AND V8 DEPENDENCIES\n========================\n\n");
    notices.push_str(
        &String::from_utf8(read(
            source_root.join("resources/RUST_THIRD_PARTY_NOTICES.txt"),
        )?)
        .context("Rust third-party notices are not UTF-8")?,
    );
    append_rust_notices(&mut notices, source_root)?;
    notices.push_str("\n\nEMBEDDED NPM PACKAGES\n=====================\n");
    for ((name, version), (license, package_dir, attribution)) in packages {
        notices.push_str(&format!("\n--- {name} {version} ({license}) ---\n"));
        notices.push_str(&attribution);
        let mut license_files = Vec::new();
        for entry in read_dir(&package_dir)
            .with_context(|| format!("read npm package {}", package_dir.display()))?
        {
            let entry = entry.context("read npm package entry")?;
            if !entry
                .file_type()
                .with_context(|| format!("read file type for {}", entry.path().display()))?
                .is_file()
            {
                continue;
            }
            let filename = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if filename.starts_with("license")
                || filename.starts_with("licence")
                || filename.starts_with("copying")
                || filename.starts_with("notice")
            {
                license_files.push(entry.path());
            }
        }
        license_files.sort();
        if license_files.is_empty() {
            notices.push_str(
                "The package does not ship a separate license file. See the identified license text elsewhere in this notice.\n",
            );
        }
        for path in license_files {
            notices.push_str(&format!(
                "\n[{}]\n",
                path.file_name().unwrap().to_string_lossy()
            ));
            notices.push_str(
                &String::from_utf8(read(&path)?).with_context(|| {
                    format!("npm license file is not UTF-8: {}", path.display())
                })?,
            );
            if !notices.ends_with('\n') {
                notices.push('\n');
            }
        }
    }
    notices.push_str(
        "\nKUROMOJI DICTIONARY\n===================\n\n--- kuromoji 0.1.2 / LICENSE-2.0.txt ---\n",
    );
    notices.push_str(
        &String::from_utf8(read(
            source_root.join("resources/kuromoji/LICENSE-2.0.txt"),
        )?)
        .context("Kuromoji license is not UTF-8")?,
    );
    notices.push_str("\n--- kuromoji 0.1.2 / NOTICE.md ---\n");
    notices.push_str(
        &String::from_utf8(read(source_root.join("resources/kuromoji/NOTICE.md"))?)
            .context("Kuromoji notice is not UTF-8")?,
    );
    write(out_dir.join("THIRD_PARTY_NOTICES.txt"), notices)
        .context("write embedded third-party notices")?;
    Ok(())
}

fn copy_dictionaries(root: &Path, out_dir: &Path) -> Result<()> {
    let source_dir = root.join("node_modules/kuromoji/dict");
    let output_dir = out_dir.join("kuromoji");
    if output_dir.exists() {
        remove_dir_all(&output_dir).with_context(|| format!("remove {}", output_dir.display()))?;
    }
    create_dir_all(&output_dir).with_context(|| format!("create {}", output_dir.display()))?;

    let mut copied = 0;
    for entry in read_dir(&source_dir)
        .with_context(|| format!("read Kuromoji dictionaries from {}", source_dir.display()))?
    {
        let entry = entry.context("read Kuromoji dictionary entry")?;
        let source = entry.path();
        if source.extension().is_none_or(|extension| extension != "gz") {
            continue;
        }
        let output = output_dir.join(entry.file_name());
        copy(&source, &output)
            .with_context(|| format!("copy {} to {}", source.display(), output.display()))?;
        copied += 1;
    }
    if copied == 0 {
        bail!("no Kuromoji dictionaries found in {}", source_dir.display());
    }
    Ok(())
}

async fn bundle(root: &Path, config: &Value, registry_path: &Path) -> Result<BundleOutput> {
    let absolute = |path: &str| root.join(path).to_string_lossy().into_owned();
    let dictionary_base_loader =
        canonicalize(root.join("node_modules/kuromoji/src/loader/DictionaryLoader.js"))
            .context("locate the installed Kuromoji dictionary loader")?
            .to_string_lossy()
            .into_owned();
    let aliases = [
        (
            REGISTRY_SPECIFIER,
            registry_path
                .to_str()
                .context("generated registry path is not UTF-8")?,
        ),
        ("assert", "js/shim/assert.cjs"),
        ("node:assert", "js/shim/assert.cjs"),
        ("path", "js/shim/path.ts"),
        ("node:path", "js/shim/path.ts"),
        ("os", "js/shim/os.ts"),
        ("node:os", "js/shim/os.ts"),
        ("fs", "js/shim/fs.ts"),
        ("node:fs", "js/shim/fs.ts"),
    ]
    .into_iter()
    .map(|(name, path)| (name.to_string(), vec![Some(absolute(path))]))
    .collect();
    let plugin = TextlintBundlePlugin {
        dictionary_loader: absolute("js/embedded-dictionary-loader.cjs"),
        dictionary_base_loader,
        conjunctive_particle_rule: absolute(
            "node_modules/textlint-rule-no-doubled-conjunctive-particle-ga/lib/no-doubled-conjunctive-particle-ga.js",
        ),
    };
    let options = BundlerOptions {
        cwd: Some(root.to_path_buf()),
        input: Some(vec![InputItem {
            name: None,
            import: "js/index.ts".into(),
        }]),
        platform: Some(Platform::Browser),
        format: Some(OutputFormat::Iife),
        name: Some("__textlintV8Bundle".into()),
        resolve: Some(ResolveOptions {
            alias: Some(aliases),
            ..Default::default()
        }),
        define: Some(
            [(
                "__TEXTLINT_V8_CONFIG__".into(),
                to_string(config).context("serialize textlint config")?,
            )]
            .into_iter()
            .collect(),
        ),
        code_splitting: Some(CodeSplittingMode::Bool(false)),
        transform: Some(BundlerTransformOptions {
            target: Some(Either::Left("es2020".into())),
            ..Default::default()
        }),
        ..Default::default()
    };

    let mut bundler = Bundler::with_plugins(options, vec![Arc::new(plugin)])
        .context("create Rolldown bundler")?;
    let output = bundler
        .generate()
        .await
        .context("generate textlint bundle with Rolldown")?;
    for warning in output.warnings {
        println!("cargo:warning=Rolldown: {warning}");
    }
    if output.assets.len() != 1 {
        bail!(
            "Rolldown emitted {} outputs; expected one JavaScript chunk",
            output.assets.len()
        );
    }
    let asset = &output.assets[0];
    let bytes = asset.content_as_bytes();
    if bytes.is_empty()
        || !bytes
            .windows(b"globalThis.textlintV8".len())
            .any(|part| part == b"globalThis.textlintV8")
    {
        bail!("generated bundle does not install globalThis.textlintV8");
    }
    let Output::Chunk(chunk) = asset else {
        bail!("Rolldown emitted an asset; expected a JavaScript chunk");
    };
    let module_ids = chunk
        .modules
        .keys
        .iter()
        .zip(&chunk.modules.values)
        .filter(|(_, module)| module.rendered_length() > 0)
        .map(|(module_id, _)| Path::new(module_id.as_str()))
        .filter(|path| path.exists())
        .map(canonicalize)
        .collect::<std::io::Result<Vec<_>>>()
        .context("canonicalize bundled module paths")?;
    Ok(BundleOutput {
        bytes: bytes.to_vec(),
        module_ids,
    })
}

fn main() -> Result<()> {
    let root = var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .context("CARGO_MANIFEST_DIR is not set")?;
    let out_dir = var_os("OUT_DIR")
        .map(PathBuf::from)
        .context("OUT_DIR is not set")?;
    let config_path = var_os("TEXTLINT_V8_CONFIG")
        .map_or_else(|| root.join("textlint-v8.config.json"), PathBuf::from);
    let update_lockfile = match var_os("TEXTLINT_V8_UPDATE_LOCKFILE") {
        None => false,
        Some(value) if value == "1" => true,
        Some(value) => bail!(
            "TEXTLINT_V8_UPDATE_LOCKFILE must be exactly 1 when set, got {}",
            value.to_string_lossy()
        ),
    };

    println!("cargo:rerun-if-env-changed=TEXTLINT_V8_CONFIG");
    println!("cargo:rerun-if-env-changed=TEXTLINT_V8_UPDATE_LOCKFILE");
    for path in ["package.json", "pnpm-lock.yaml", "js", "resources"] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-changed={}", config_path.display());

    let config: Value = from_slice(
        &read(&config_path)
            .with_context(|| format!("read textlint config {}", config_path.display()))?,
    )
    .with_context(|| format!("parse textlint config {}", config_path.display()))?;
    let manifest: Value = from_slice(
        &read(root.join("package.json")).context("read package.json for rule registry")?,
    )
    .context("parse package.json for rule registry")?;
    let packages = rule_packages(&config, &manifest)?;
    let workspace = prepare_workspace(&root, &out_dir)?;
    let registry_path = generate_registry(&packages, &workspace)?;

    let runtime = Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("create build runtime")?;
    let bundle = runtime.block_on(async {
        install_dependencies(&workspace, update_lockfile).await?;
        bundle(&workspace, &config, &registry_path).await
    })?;

    if update_lockfile {
        copy(
            workspace.join("pnpm-lock.yaml"),
            root.join("pnpm-lock.yaml"),
        )
        .context("copy explicitly updated pnpm lockfile to the source tree")?;
    }
    copy_dictionaries(&workspace, &out_dir)?;
    generate_notices(&workspace, &root, &out_dir, &bundle.module_ids)?;
    let output = out_dir.join("textlint-v8.js");
    write(&output, bundle.bytes).with_context(|| format!("write {}", output.display()))?;
    println!("cargo:warning=npm dependencies installed with {PNPM_SOURCE}");
    println!(
        "cargo:warning=embedded textlint config: {}",
        config_path.display()
    );
    Ok(())
}
