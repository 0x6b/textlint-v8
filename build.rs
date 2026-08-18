use std::{
    borrow::Cow,
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
    config.store_dir = root.join("target/pnpm-store").into();
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

async fn bundle(root: &Path, config: &Value, registry_path: &Path) -> Result<Vec<u8>> {
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
        ("assert", "js/assert-shim.cjs"),
        ("node:assert", "js/assert-shim.cjs"),
        ("path", "js/path-shim.ts"),
        ("node:path", "js/path-shim.ts"),
        ("os", "js/os-shim.ts"),
        ("node:os", "js/os-shim.ts"),
        ("fs", "js/fs-shim.ts"),
        ("node:fs", "js/fs-shim.ts"),
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
    Ok(bytes.to_vec())
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
    for path in ["package.json", "pnpm-lock.yaml", "js"] {
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
    let registry_path = generate_registry(&packages, &out_dir)?;

    let runtime = Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("create build runtime")?;
    let bytes = runtime.block_on(async {
        install_dependencies(&root, update_lockfile).await?;
        bundle(&root, &config, &registry_path).await
    })?;

    copy_dictionaries(&root, &out_dir)?;
    let output = out_dir.join("textlint-v8.js");
    write(&output, bytes).with_context(|| format!("write {}", output.display()))?;
    println!("cargo:warning=npm dependencies installed with {PNPM_SOURCE}");
    println!(
        "cargo:warning=embedded textlint config: {}",
        config_path.display()
    );
    Ok(())
}
