use std::{borrow::Cow, env, fs, path::Path, sync::Arc};

use anyhow::{Context, Result, bail};
use pnpm_config::Config;
use pnpm_lockfile::{LazyLockfile, Lockfile, MaybeLazyLockfile};
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
use serde_json::Value;

const PNPM_SOURCE: &str = "pnpm/pnpm@87b9504492d879c05c550def1c8058214ebaac83";

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

fn validate_config(config: &Value) -> Result<()> {
    let config = config
        .as_object()
        .context("textlint config must be an object")?;
    let rules = config
        .get("rules")
        .and_then(Value::as_object)
        .context("textlint config must contain a rules object")?;
    let available = [
        "@0x6b/no-emoji",
        "@0x6b/no-emphasis",
        "@0x6b/no-hr-before-heading",
        "@0x6b/no-numbered-headings-and-bullets",
        "@0x6b/no-smart-quotes",
        "@0x6b/normalize-whitespaces",
        "@textlint-ja/preset-ai-writing",
        "preset-ja-technical-writing",
    ];
    for (rule_id, options) in rules {
        if !available.contains(&rule_id.as_str()) {
            bail!("unknown bundled textlint rule: {rule_id}");
        }
        if !options.is_boolean() && !options.is_object() {
            bail!("configuration for {rule_id} must be a boolean or object");
        }
    }
    Ok(())
}

async fn install_dependencies(root: &Path) -> Result<()> {
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
    let http_client = Arc::new(pnpm_network::ThrottledClient::default());
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
        frozen_lockfile: true,
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

async fn bundle(root: &Path, config: &Value) -> Result<Vec<u8>> {
    let absolute = |path: &str| root.join(path).to_string_lossy().into_owned();
    let dictionary_base_loader =
        fs::canonicalize(root.join("node_modules/kuromoji/src/loader/DictionaryLoader.js"))
            .context("locate the installed Kuromoji dictionary loader")?
            .to_string_lossy()
            .into_owned();
    let aliases = [
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
                serde_json::to_string(config).context("serialize textlint config")?,
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
    let root = env::var_os("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .context("CARGO_MANIFEST_DIR is not set")?;
    let config_path = env::var_os("TEXTLINT_V8_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("textlint-v8.config.json"));

    println!("cargo:rerun-if-env-changed=TEXTLINT_V8_CONFIG");
    for path in [
        "package.json",
        "pnpm-lock.yaml",
        "js",
        "textlint-v8.config.json",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    if !config_path.starts_with(&root) {
        println!("cargo:rerun-if-changed={}", config_path.display());
    }

    let config: Value = serde_json::from_slice(
        &fs::read(&config_path)
            .with_context(|| format!("read textlint config {}", config_path.display()))?,
    )
    .with_context(|| format!("parse textlint config {}", config_path.display()))?;
    validate_config(&config)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("create build runtime")?;
    let bytes = runtime.block_on(async {
        install_dependencies(&root).await?;
        bundle(&root, &config).await
    })?;

    let output = root.join("dist/textlint-v8.js");
    fs::create_dir_all(output.parent().expect("dist output has a parent"))?;
    fs::write(&output, bytes).with_context(|| format!("write {}", output.display()))?;
    println!("cargo:warning=npm dependencies installed with {PNPM_SOURCE}");
    println!(
        "cargo:warning=embedded textlint config: {}",
        config_path.display()
    );
    Ok(())
}
