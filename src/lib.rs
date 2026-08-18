use std::{cell::RefCell, io::Read as _, sync::Once};

use anyhow::{Context as _, Result, anyhow, bail};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_string};
use v8::{
    ArrayBuffer, Context, ContextScope, CreateParams, Exception, Function,
    FunctionCallbackArguments, Global, Isolate, Local, MicrotasksPolicy, Object, OwnedIsolate,
    PinScope, Promise, PromiseState, ReturnValue, Script, V8, new_default_platform,
};

const TEXTLINT_BUNDLE: &str = include_str!(concat!(env!("OUT_DIR"), "/textlint-v8.js"));
const V8_PRELUDE: &str = r"
globalThis.console = {
  log() {}, debug() {}, info() {}, warn() {}, error() {}
};
";
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
    context: Global<Context>,
    isolate: OwnedIsolate,
}

fn compressed_dictionary(filename: &str) -> Option<&'static [u8]> {
    Some(match filename {
        "base.dat.gz" => include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/base.dat.gz")),
        "check.dat.gz" => include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/check.dat.gz")),
        "tid.dat.gz" => include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/tid.dat.gz")),
        "tid_pos.dat.gz" => {
            include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/tid_pos.dat.gz"))
        }
        "tid_map.dat.gz" => {
            include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/tid_map.dat.gz"))
        }
        "cc.dat.gz" => include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/cc.dat.gz")),
        "unk.dat.gz" => include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/unk.dat.gz")),
        "unk_pos.dat.gz" => {
            include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/unk_pos.dat.gz"))
        }
        "unk_map.dat.gz" => {
            include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/unk_map.dat.gz"))
        }
        "unk_char.dat.gz" => {
            include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/unk_char.dat.gz"))
        }
        "unk_compat.dat.gz" => {
            include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/unk_compat.dat.gz"))
        }
        "unk_invoke.dat.gz" => {
            include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/unk_invoke.dat.gz"))
        }
        _ => return None,
    })
}

fn throw_error(scope: &mut PinScope, message: &str) {
    let message = v8::String::new(scope, message).expect("short error message");
    let error = Exception::error(scope, message);
    scope.throw_exception(error);
}

fn load_dictionary(
    scope: &mut PinScope,
    args: FunctionCallbackArguments,
    mut return_value: ReturnValue,
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

    let store = ArrayBuffer::new_backing_store_from_vec(decoded).make_shared();
    let buffer = ArrayBuffer::with_backing_store(scope, &store);
    return_value.set(buffer.into());
}

fn run_script(scope: &mut PinScope, source: &str, label: &str) -> Result<()> {
    let source = v8::String::new(scope, source)
        .ok_or_else(|| anyhow!("failed to allocate V8 source for {label}"))?;
    let script =
        Script::compile(scope, source, None).ok_or_else(|| anyhow!("failed to compile {label}"))?;
    script
        .run(scope)
        .ok_or_else(|| anyhow!("failed to evaluate {label}"))?;
    Ok(())
}

impl Textlint {
    pub fn new() -> Result<Self> {
        V8_INIT.call_once(|| {
            let platform = new_default_platform(0, false).make_shared();
            V8::initialize_platform(platform);
            V8::initialize();
        });

        let mut isolate = Isolate::new(CreateParams::default());
        isolate.set_microtasks_policy(MicrotasksPolicy::Explicit);
        let context = {
            v8::scope!(let scope, &mut isolate);
            let context = Context::new(scope, Default::default());
            let scope = &mut ContextScope::new(scope, context);

            let loader = Function::new(scope, load_dictionary)
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
            Global::new(scope, context)
        };

        Ok(Self {
            state: RefCell::new(V8State { context, isolate }),
        })
    }

    pub fn lint(&self, text: &str, file_path: &str) -> Result<LintResult> {
        let request = to_string(&LintRequest { text, file_path })?;
        let mut state = self.state.borrow_mut();
        let V8State { context, isolate } = &mut *state;
        v8::scope!(let scope, isolate);
        let context = Local::new(scope, &*context);
        let scope = &mut ContextScope::new(scope, context);

        let api_name = v8::String::new(scope, "textlintV8").unwrap();
        let api = context
            .global(scope)
            .get(scope, api_name.into())
            .ok_or_else(|| anyhow!("textlintV8 is not defined"))?;
        let api = v8::Local::<Object>::try_from(api)
            .map_err(|_| anyhow!("textlintV8 is not an object"))?;
        let lint_name = v8::String::new(scope, "lint").unwrap();
        let lint = api
            .get(scope, lint_name.into())
            .ok_or_else(|| anyhow!("textlintV8.lint is not defined"))?;
        let lint = v8::Local::<Function>::try_from(lint)
            .map_err(|_| anyhow!("textlintV8.lint is not a function"))?;
        let request = v8::String::new(scope, &request)
            .ok_or_else(|| anyhow!("failed to allocate lint request"))?;
        let promise = lint
            .call(scope, api.into(), &[request.into()])
            .ok_or_else(|| anyhow!("textlintV8.lint threw an exception"))?;
        let promise = v8::Local::<Promise>::try_from(promise)
            .map_err(|_| anyhow!("textlintV8.lint did not return a Promise"))?;

        scope.perform_microtask_checkpoint();
        match promise.state() {
            PromiseState::Pending => bail!("textlint Promise remained pending"),
            PromiseState::Rejected => {
                let error = promise.result(scope).to_rust_string_lossy(scope);
                bail!("textlint failed: {error}");
            }
            PromiseState::Fulfilled => {}
        }

        let output = promise.result(scope).to_rust_string_lossy(scope);
        from_str(&output).context("textlint returned invalid JSON")
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
