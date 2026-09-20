use std::{
    borrow::Cow,
    collections::BTreeMap,
    fs::canonicalize,
    io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
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
use serde::{Deserialize, Serialize};
use serde_json::{Value, to_string};

const REGISTRY_SPECIFIER: &str = "textlint-v8:registry";

#[derive(Debug)]
pub struct RulePackage {
    id: String,
    package: String,
    preset_prefix: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct TextlintConfig {
    rules: BTreeMap<String, RuleConfig>,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum RuleConfig {
    Enabled(bool),
    Options(BTreeMap<String, Value>),
}

#[derive(Deserialize)]
pub struct PackageManifest {
    dependencies: BTreeMap<String, String>,
}

#[derive(Debug)]
struct TextlintBundlePlugin {
    dictionary_loader: String,
    dictionary_base_loader: String,
    conjunctive_particle_rule: String,
}

pub struct BundleOutput {
    pub bytes: Vec<u8>,
    pub module_ids: Vec<PathBuf>,
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
            && args.importer.is_some_and(|importer| importer.contains("/kuromoji/"))
        {
            return Ok(Some(HookResolveIdOutput::from_id(self.dictionary_loader.clone())));
        }
        if args.specifier == "kuromoji/src/loader/DictionaryLoader.js"
            && args.importer == Some(self.dictionary_loader.as_str())
        {
            return Ok(Some(HookResolveIdOutput::from_id(self.dictionary_base_loader.clone())));
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

pub fn rule_packages(
    config: &TextlintConfig,
    manifest: &PackageManifest,
) -> Result<Vec<RulePackage>> {
    let mut packages = Vec::with_capacity(config.rules.len());

    for rule_id in config.rules.keys() {
        let (package, preset_prefix) = package_for_rule_id(rule_id)?;
        let version = manifest.dependencies.get(&package).with_context(|| {
            format!("textlint rule {rule_id} requires package.json dependency {package}")
        })?;
        Version::parse(version).with_context(|| {
            format!(
                "textlint rule dependency {package} must use a fixed semantic version, got {version}"
            )
        })?;
        packages.push(RulePackage { id: rule_id.clone(), package, preset_prefix });
    }
    Ok(packages)
}

pub fn generate_registry(packages: &[RulePackage], out_dir: &Path) -> Result<PathBuf> {
    use std::{fmt::Write as _, fs::write};

    let mut source = String::new();
    for (index, rule) in packages.iter().enumerate() {
        writeln!(
            source,
            "import rule{index} from {};",
            to_string(&rule.package).context("serialize rule package name")?
        )?;
    }
    source.push_str("\nexport const standaloneRules = [\n");
    for (index, rule) in packages.iter().enumerate() {
        if rule.preset_prefix.is_none() {
            writeln!(
                source,
                "  {{ ruleId: {}, rule: rule{index} }},",
                to_string(&rule.id).context("serialize rule ID")?
            )?;
        }
    }
    source.push_str("] as const;\n\nexport const presetDefinitions = [\n");
    for (index, rule) in packages.iter().enumerate() {
        if let Some(prefix) = &rule.preset_prefix {
            writeln!(
                source,
                "  {{ presetId: {}, ruleIdPrefix: {}, preset: rule{index} }},",
                to_string(&rule.id).context("serialize preset ID")?,
                to_string(prefix).context("serialize preset rule prefix")?
            )?;
        }
    }
    source.push_str("] as const;\n");

    let path = out_dir.join("textlint-rule-registry.ts");
    write(&path, source).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub async fn bundle(
    root: &Path,
    config: &TextlintConfig,
    registry_path: &Path,
) -> Result<BundleOutput> {
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
        ("util", "js/shim/util.ts"),
        ("node:util", "js/shim/util.ts"),
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
        input: Some(vec![InputItem { name: None, import: "js/index.ts".into() }]),
        platform: Some(Platform::Browser),
        format: Some(OutputFormat::Iife),
        name: Some("__textlintV8Bundle".into()),
        resolve: Some(ResolveOptions { alias: Some(aliases), ..Default::default() }),
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

    let mut bundler = Bundler::with_plugins(options, vec![Plugin::new_shared(plugin)])
        .context("create Rolldown bundler")?;
    let output = bundler
        .generate()
        .await
        .context("generate textlint bundle with Rolldown")?;
    for warning in output.warnings {
        println!("cargo:warning=Rolldown: {warning}");
    }
    if output.assets.len() != 1 {
        bail!("Rolldown emitted {} outputs; expected one JavaScript chunk", output.assets.len());
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
        .collect::<io::Result<Vec<_>>>()
        .context("canonicalize bundled module paths")?;
    Ok(BundleOutput { bytes: bytes.to_vec(), module_ids })
}
