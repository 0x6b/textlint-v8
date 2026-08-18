use std::{io::Read as _, marker::PhantomData, rc::Rc, sync::Once};

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_string};
use thiserror::Error;
use v8::{
    ArrayBuffer, Context, ContextScope, CreateParams, Exception, Function,
    FunctionCallbackArguments, Global, Isolate, Local, MicrotasksPolicy, Object, OwnedIsolate,
    PinScope, Promise, PromiseState, ReturnValue, Script, V8, Value, new_default_platform,
};

const TEXTLINT_BUNDLE: &str = include_str!(concat!(env!("OUT_DIR"), "/textlint-v8.js"));
const THIRD_PARTY_NOTICES: &str =
    include_str!(concat!(env!("OUT_DIR"), "/THIRD_PARTY_NOTICES.txt"));
const V8_PRELUDE: &str = r"
globalThis.console = {
  log() {}, debug() {}, info() {}, warn() {}, error() {}
};
";
static V8_INIT: Once = Once::new();

/// Errors returned by the public textlint API.
#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to serialize the lint request")]
    RequestSerialization(#[source] serde_json::Error),
    #[error("V8 operation failed while attempting to {operation}: {message}")]
    V8 {
        operation: &'static str,
        message: String,
        stack: Option<String>,
    },
    #[error("textlint JavaScript failed: {message}")]
    JavaScript {
        message: String,
        stack: Option<String>,
    },
    #[error("the textlint Promise remained pending")]
    PromisePending,
    #[error("textlint returned invalid JSON")]
    InvalidResponse {
        response: String,
        #[source]
        source: serde_json::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Returns the license and attribution notices embedded in this build.
pub const fn third_party_notices() -> &'static str {
    THIRD_PARTY_NOTICES
}

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
    state: V8State,
    // V8 isolates are thread-affine. Keep that constraint explicit even if
    // rusty_v8 changes its auto-trait implementations.
    _not_send_or_sync: PhantomData<Rc<()>>,
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

fn v8_error(
    scope: &mut PinScope,
    operation: &'static str,
    exception: Option<Local<Value>>,
    stack: Option<Local<Value>>,
) -> Error {
    Error::V8 {
        operation,
        message: exception
            .map(|value| value.to_rust_string_lossy(scope))
            .unwrap_or_else(|| "V8 returned no exception details".to_owned()),
        stack: stack.map(|value| value.to_rust_string_lossy(scope)),
    }
}

fn javascript_error(scope: &mut PinScope, value: Local<Value>) -> Error {
    let message = value.to_rust_string_lossy(scope);
    let stack = Local::<Object>::try_from(value).ok().and_then(|object| {
        let name = v8::String::new(scope, "stack")?;
        object
            .get(scope, name.into())
            .map(|stack| stack.to_rust_string_lossy(scope))
    });
    Error::JavaScript { message, stack }
}

fn run_script(scope: &mut PinScope, source: &str, operation: &'static str) -> Result<()> {
    v8::tc_scope!(let scope, scope);
    let Some(source) = v8::String::new(scope, source) else {
        let exception = scope.exception();
        let stack = scope.stack_trace();
        return Err(v8_error(scope, operation, exception, stack));
    };
    let Some(script) = Script::compile(scope, source, None) else {
        let exception = scope.exception();
        let stack = scope.stack_trace();
        return Err(v8_error(scope, operation, exception, stack));
    };
    if script.run(scope).is_none() {
        let exception = scope.exception();
        let stack = scope.stack_trace();
        return Err(v8_error(scope, operation, exception, stack));
    }
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

            let loader = Function::new(scope, load_dictionary).ok_or_else(|| Error::V8 {
                operation: "create the dictionary loader",
                message: "V8 returned no function".to_owned(),
                stack: None,
            })?;
            let loader_name =
                v8::String::new(scope, "__loadKuromojiDictionary").ok_or_else(|| Error::V8 {
                    operation: "allocate the dictionary loader name",
                    message: "V8 returned no string".to_owned(),
                    stack: None,
                })?;
            if !context
                .global(scope)
                .set(scope, loader_name.into(), loader.into())
                .unwrap_or(false)
            {
                return Err(Error::V8 {
                    operation: "install the dictionary loader",
                    message: "V8 rejected the global property".to_owned(),
                    stack: None,
                });
            }

            run_script(scope, V8_PRELUDE, "evaluate the V8 prelude")?;
            run_script(
                scope,
                TEXTLINT_BUNDLE,
                "evaluate the embedded textlint bundle",
            )?;
            Global::new(scope, context)
        };

        Ok(Self {
            state: V8State { context, isolate },
            _not_send_or_sync: PhantomData,
        })
    }

    pub fn lint(&mut self, text: &str, file_path: &str) -> Result<LintResult> {
        let request =
            to_string(&LintRequest { text, file_path }).map_err(Error::RequestSerialization)?;
        let V8State { context, isolate } = &mut self.state;
        v8::scope!(let scope, isolate);
        let context = Local::new(scope, &*context);
        let scope = &mut ContextScope::new(scope, context);
        v8::tc_scope!(let scope, scope);

        let result = (|| {
            let api_name = v8::String::new(scope, "textlintV8")?;
            let api = context.global(scope).get(scope, api_name.into())?;
            let api = v8::Local::<Object>::try_from(api).ok()?;
            let lint_name = v8::String::new(scope, "lint")?;
            let lint = api.get(scope, lint_name.into())?;
            let lint = v8::Local::<Function>::try_from(lint).ok()?;
            let request = v8::String::new(scope, &request)?;
            lint.call(scope, api.into(), &[request.into()])
        })();
        let Some(result) = result else {
            let exception = scope.exception();
            let stack = scope.stack_trace();
            return Err(v8_error(scope, "invoke textlintV8.lint", exception, stack));
        };
        let promise = v8::Local::<Promise>::try_from(result).map_err(|_| Error::V8 {
            operation: "invoke textlintV8.lint",
            message: "textlintV8.lint did not return a Promise".to_owned(),
            stack: None,
        })?;

        scope.perform_microtask_checkpoint();
        match promise.state() {
            PromiseState::Pending => return Err(Error::PromisePending),
            PromiseState::Rejected => {
                let error = promise.result(scope);
                return Err(javascript_error(scope, error));
            }
            PromiseState::Fulfilled => {}
        }

        let output = promise.result(scope).to_rust_string_lossy(scope);
        from_str(&output).map_err(|source| Error::InvalidResponse {
            response: output,
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::{Textlint, third_party_notices};

    assert_not_impl_any!(Textlint: Send, Sync);

    #[test]
    fn reports_standalone_and_preset_rules() {
        let mut textlint = Textlint::new().unwrap();
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

    #[test]
    fn embeds_third_party_notices() {
        let notices = third_party_notices();
        assert!(notices.contains("kuromoji 0.1.2"));
        assert!(notices.contains("@textlint/kernel 15.5.2"));
        assert!(notices.contains("mecab-ipadic-2.7.0-20070801"));
    }
}
