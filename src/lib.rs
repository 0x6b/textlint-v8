use std::{cell::RefCell, io::Read as _, sync::Once};

use anyhow::{Context as _, Result, anyhow, bail};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};

const TEXTLINT_BUNDLE: &str = include_str!("../dist/textlint-v8.js");
const V8_PRELUDE: &str = r#"
globalThis.console = {
  log() {}, debug() {}, info() {}, warn() {}, error() {}
};
"#;
static V8_INIT: Once = Once::new();

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LintRequest<'a> {
    text: &'a str,
    file_path: &'a str,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LintResult {
    pub file_path: String,
    pub messages: Vec<LintMessage>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LintMessage {
    pub rule_id: String,
    pub message: String,
    pub index: usize,
    pub line: usize,
    pub column: usize,
    pub severity: usize,
    pub range: Vec<usize>,
}

pub struct Textlint {
    state: RefCell<V8State>,
}

struct V8State {
    context: v8::Global<v8::Context>,
    isolate: v8::OwnedIsolate,
}

fn compressed_dictionary(filename: &str) -> Option<&'static [u8]> {
    Some(match filename {
        "base.dat.gz" => include_bytes!("../resources/kuromoji/base.dat.gz"),
        "check.dat.gz" => include_bytes!("../resources/kuromoji/check.dat.gz"),
        "tid.dat.gz" => include_bytes!("../resources/kuromoji/tid.dat.gz"),
        "tid_pos.dat.gz" => include_bytes!("../resources/kuromoji/tid_pos.dat.gz"),
        "tid_map.dat.gz" => include_bytes!("../resources/kuromoji/tid_map.dat.gz"),
        "cc.dat.gz" => include_bytes!("../resources/kuromoji/cc.dat.gz"),
        "unk.dat.gz" => include_bytes!("../resources/kuromoji/unk.dat.gz"),
        "unk_pos.dat.gz" => include_bytes!("../resources/kuromoji/unk_pos.dat.gz"),
        "unk_map.dat.gz" => include_bytes!("../resources/kuromoji/unk_map.dat.gz"),
        "unk_char.dat.gz" => include_bytes!("../resources/kuromoji/unk_char.dat.gz"),
        "unk_compat.dat.gz" => include_bytes!("../resources/kuromoji/unk_compat.dat.gz"),
        "unk_invoke.dat.gz" => include_bytes!("../resources/kuromoji/unk_invoke.dat.gz"),
        _ => return None,
    })
}

fn throw_error(scope: &mut v8::PinScope, message: &str) {
    let message = v8::String::new(scope, message).expect("short error message");
    let error = v8::Exception::error(scope, message);
    scope.throw_exception(error);
}

fn load_dictionary(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let filename = args.get(0).to_rust_string_lossy(scope);
    let Some(compressed) = compressed_dictionary(&filename) else {
        throw_error(scope, &format!("unknown dictionary file: {filename}"));
        return;
    };

    let mut decoded = Vec::new();
    if let Err(error) = GzDecoder::new(compressed).read_to_end(&mut decoded) {
        throw_error(scope, &format!("failed to decompress {filename}: {error}"));
        return;
    }

    let store = v8::ArrayBuffer::new_backing_store_from_vec(decoded).make_shared();
    let buffer = v8::ArrayBuffer::with_backing_store(scope, &store);
    return_value.set(buffer.into());
}

fn run_script(scope: &mut v8::PinScope, source: &str, label: &str) -> Result<()> {
    let source = v8::String::new(scope, source)
        .ok_or_else(|| anyhow!("failed to allocate V8 source for {label}"))?;
    let script = v8::Script::compile(scope, source, None)
        .ok_or_else(|| anyhow!("failed to compile {label}"))?;
    script
        .run(scope)
        .ok_or_else(|| anyhow!("failed to evaluate {label}"))?;
    Ok(())
}

impl Textlint {
    pub fn new() -> Result<Self> {
        V8_INIT.call_once(|| {
            let platform = v8::new_default_platform(0, false).make_shared();
            v8::V8::initialize_platform(platform);
            v8::V8::initialize();
        });

        let mut isolate = v8::Isolate::new(v8::CreateParams::default());
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
        let context = {
            v8::scope!(let scope, &mut isolate);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);

            let loader = v8::Function::new(scope, load_dictionary)
                .ok_or_else(|| anyhow!("failed to create dictionary loader"))?;
            let loader_name = v8::String::new(scope, "__loadKuromojiDictionary")
                .ok_or_else(|| anyhow!("failed to allocate dictionary loader name"))?;
            if !context
                .global(scope)
                .set(scope, loader_name.into(), loader.into())
                .unwrap_or(false)
            {
                bail!("failed to install dictionary loader");
            }

            run_script(scope, V8_PRELUDE, "V8 prelude")?;
            run_script(scope, TEXTLINT_BUNDLE, "embedded textlint bundle")?;
            v8::Global::new(scope, context)
        };

        Ok(Self {
            state: RefCell::new(V8State { context, isolate }),
        })
    }

    pub fn lint(&self, text: &str, file_path: &str) -> Result<LintResult> {
        let request = serde_json::to_string(&LintRequest { text, file_path })?;
        let mut state = self.state.borrow_mut();
        let V8State { context, isolate } = &mut *state;
        v8::scope!(let scope, isolate);
        let context = v8::Local::new(scope, &*context);
        let scope = &mut v8::ContextScope::new(scope, context);

        let api_name = v8::String::new(scope, "textlintV8").unwrap();
        let api = context
            .global(scope)
            .get(scope, api_name.into())
            .ok_or_else(|| anyhow!("textlintV8 is not defined"))?;
        let api = v8::Local::<v8::Object>::try_from(api)
            .map_err(|_| anyhow!("textlintV8 is not an object"))?;
        let lint_name = v8::String::new(scope, "lint").unwrap();
        let lint = api
            .get(scope, lint_name.into())
            .ok_or_else(|| anyhow!("textlintV8.lint is not defined"))?;
        let lint = v8::Local::<v8::Function>::try_from(lint)
            .map_err(|_| anyhow!("textlintV8.lint is not a function"))?;
        let request = v8::String::new(scope, &request)
            .ok_or_else(|| anyhow!("failed to allocate lint request"))?;
        let promise = lint
            .call(scope, api.into(), &[request.into()])
            .ok_or_else(|| anyhow!("textlintV8.lint threw an exception"))?;
        let promise = v8::Local::<v8::Promise>::try_from(promise)
            .map_err(|_| anyhow!("textlintV8.lint did not return a Promise"))?;

        scope.perform_microtask_checkpoint();
        match promise.state() {
            v8::PromiseState::Pending => bail!("textlint Promise remained pending"),
            v8::PromiseState::Rejected => {
                let error = promise.result(scope).to_rust_string_lossy(scope);
                bail!("textlint failed: {error}");
            }
            v8::PromiseState::Fulfilled => {}
        }

        let output = promise.result(scope).to_rust_string_lossy(scope);
        serde_json::from_str(&output).context("textlint returned invalid JSON")
    }
}

#[cfg(test)]
mod tests {
    use super::Textlint;

    #[test]
    fn reports_standalone_and_preset_rules() {
        let textlint = Textlint::new().unwrap();
        let result = textlint
            .lint(
                "# 1. “見出し” 😀\n\n*強調*\n\n---\n\n## 次\n\n1. 箇条書き\n\n特殊　空白\n\n革命的な技術です。\n\nこれは見ることができないわけではない。\n",
                "sample.md",
            )
            .unwrap();
        let ids: Vec<_> = result
            .messages
            .iter()
            .map(|message| message.rule_id.as_str())
            .collect();

        assert!(ids.contains(&"@0x6b/no-emoji"));
        assert!(ids.contains(&"@0x6b/no-emphasis"));
        assert!(ids.contains(&"@0x6b/no-hr-before-heading"));
        assert!(ids.contains(&"@0x6b/no-numbered-headings-and-bullets"));
        assert!(ids.contains(&"@0x6b/no-smart-quotes"));
        assert!(ids.contains(&"@0x6b/normalize-whitespaces"));
        assert!(ids.contains(&"@textlint-ja/ai-writing/no-ai-hype-expressions"));
        assert!(ids.contains(&"ja-technical-writing/no-double-negative-ja"));

        let second_result = textlint
            .lint("試したが失敗したが、再試行した。", "second.md")
            .unwrap();
        assert!(second_result.messages.iter().any(|message| {
            message.rule_id == "ja-technical-writing/no-doubled-conjunctive-particle-ga"
        }));
    }
}
