//! Run an embedded [textlint](https://textlint.org/) bundle from Rust.
//!
//! [`Textlint`] lints and fixes Markdown without requiring Node.js or files from
//! an npm installation at runtime. A `Textlint` instance owns a V8 isolate and
//! can be reused for multiple documents.
//!
//! # Example
//!
//! ```no_run
//! use textlint_v8::Textlint;
//!
//! let mut textlint = Textlint::new()?;
//! let result = textlint.lint("Some Markdown 😀", "document.md")?;
//! for message in result.messages {
//!     println!("{}:{}: {}", message.line, message.column, message.message);
//! }
//! # Ok::<(), textlint_v8::Error>(())
//! ```
#![deny(missing_docs)]

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
    /// A request could not be serialized before it was passed to textlint.
    #[error("failed to serialize the textlint request")]
    RequestSerialization(#[source] serde_json::Error),
    /// V8 failed while preparing or invoking the embedded textlint bundle.
    #[error("V8 operation failed while attempting to {operation}: {message}")]
    V8 {
        /// The operation that failed.
        operation: &'static str,
        /// V8's description of the failure.
        message: String,
        /// The JavaScript stack trace, when V8 provided one.
        stack: Option<String>,
    },
    /// The embedded textlint JavaScript rejected an operation.
    #[error("textlint JavaScript failed: {message}")]
    JavaScript {
        /// The value with which the JavaScript promise was rejected.
        message: String,
        /// The JavaScript stack trace, when the rejected value provided one.
        stack: Option<String>,
    },
    /// A textlint operation did not settle during V8's microtask checkpoint.
    #[error("the textlint Promise remained pending")]
    PromisePending,
    /// Textlint returned a response that did not match the expected JSON shape.
    #[error("textlint returned invalid JSON")]
    InvalidResponse {
        /// The unparsed response returned by textlint.
        response: String,
        /// The JSON deserialization error.
        #[source]
        source: serde_json::Error,
    },
}

/// A result returned by this crate's textlint operations.
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FormatRequest<'a, T: ?Sized> {
    formatter_name: &'a str,
    results: &'a T,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// The diagnostics produced by linting one document.
pub struct LintResult {
    /// The path supplied to [`Textlint::lint`].
    pub file_path: String,
    /// The diagnostics reported for the document.
    pub messages: Vec<LintMessage>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// The output and diagnostics produced by fixing one document.
pub struct FixResult {
    /// The path supplied to [`Textlint::fix`].
    pub file_path: String,
    /// The document after textlint applied automatic fixes.
    pub output: String,
    /// All diagnostics reported during the fix operation.
    pub messages: Vec<LintMessage>,
    /// The diagnostics whose fixes were applied to [`Self::output`].
    pub applying_messages: Vec<LintMessage>,
    /// The diagnostics that remain after applying fixes.
    pub remaining_messages: Vec<LintMessage>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// A diagnostic reported by a textlint rule.
pub struct LintMessage {
    /// The identifier of the rule that reported the diagnostic.
    pub rule_id: String,
    /// The human-readable diagnostic message.
    pub message: String,
    /// The zero-based source index reported by textlint.
    pub index: usize,
    /// The one-based source line containing the diagnostic.
    pub line: usize,
    /// The one-based source column containing the diagnostic.
    pub column: usize,
    /// The textlint severity: `0` for info, `1` for warning, or `2` for error.
    pub severity: usize,
    /// The zero-based start and end source indices reported by textlint.
    pub range: Vec<usize>,
}

/// An embedded textlint runtime.
///
/// Constructing this type initializes V8 and evaluates the bundled JavaScript.
/// Reusing an instance avoids repeating that work for every document.
///
/// A `Textlint` instance is neither [`Send`] nor [`Sync`] because its V8
/// isolate is thread-affine. All operations require exclusive access to the
/// instance.
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

macro_rules! dictionary {
    ($filename:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/kuromoji/", $filename))
    };
}

fn compressed_dictionary(filename: &str) -> Option<&'static [u8]> {
    match filename {
        "base.dat.gz" => Some(dictionary!("base.dat.gz")),
        "check.dat.gz" => Some(dictionary!("check.dat.gz")),
        "tid.dat.gz" => Some(dictionary!("tid.dat.gz")),
        "tid_pos.dat.gz" => Some(dictionary!("tid_pos.dat.gz")),
        "tid_map.dat.gz" => Some(dictionary!("tid_map.dat.gz")),
        "cc.dat.gz" => Some(dictionary!("cc.dat.gz")),
        "unk.dat.gz" => Some(dictionary!("unk.dat.gz")),
        "unk_pos.dat.gz" => Some(dictionary!("unk_pos.dat.gz")),
        "unk_map.dat.gz" => Some(dictionary!("unk_map.dat.gz")),
        "unk_char.dat.gz" => Some(dictionary!("unk_char.dat.gz")),
        "unk_compat.dat.gz" => Some(dictionary!("unk_compat.dat.gz")),
        "unk_invoke.dat.gz" => Some(dictionary!("unk_invoke.dat.gz")),
        _ => None,
    }
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
    /// Creates a textlint runtime and evaluates the embedded bundle.
    ///
    /// The process-wide V8 platform is initialized only once, even when
    /// multiple runtimes are created.
    ///
    /// # Errors
    ///
    /// Returns an error if V8 cannot create the runtime or evaluate the
    /// embedded JavaScript.
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

    fn invoke<Request: Serialize + ?Sized, Response: for<'de> Deserialize<'de>>(
        &mut self,
        method: &'static str,
        request: &Request,
    ) -> Result<Response> {
        let request = to_string(request).map_err(Error::RequestSerialization)?;
        let V8State { context, isolate } = &mut self.state;
        v8::scope!(let scope, isolate);
        let context = Local::new(scope, &*context);
        let scope = &mut ContextScope::new(scope, context);
        v8::tc_scope!(let scope, scope);

        let result = (|| {
            let api_name = v8::String::new(scope, "textlintV8")?;
            let api = context.global(scope).get(scope, api_name.into())?;
            let api = v8::Local::<Object>::try_from(api).ok()?;
            let method_name = v8::String::new(scope, method)?;
            let function = api.get(scope, method_name.into())?;
            let function = v8::Local::<Function>::try_from(function).ok()?;
            let request = v8::String::new(scope, &request)?;
            function.call(scope, api.into(), &[request.into()])
        })();
        let Some(result) = result else {
            let exception = scope.exception();
            let stack = scope.stack_trace();
            return Err(v8_error(scope, method, exception, stack));
        };
        let promise = v8::Local::<Promise>::try_from(result).map_err(|_| Error::V8 {
            operation: method,
            message: format!("textlintV8.{method} did not return a Promise"),
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

    /// Lints a Markdown document.
    ///
    /// `file_path` labels the result and is passed to textlint for rule and
    /// formatter output; this method does not read the path from the file
    /// system.
    ///
    /// # Errors
    ///
    /// Returns an error if the request cannot be serialized, the embedded
    /// JavaScript fails, or its response cannot be deserialized.
    pub fn lint(&mut self, text: &str, file_path: &str) -> Result<LintResult> {
        self.invoke("lint", &LintRequest { text, file_path })
    }

    /// Applies textlint's automatic fixes to a Markdown document.
    ///
    /// `file_path` labels the result; this method neither reads from nor writes
    /// to the file system. The fixed document is returned in
    /// [`FixResult::output`].
    ///
    /// # Errors
    ///
    /// Returns an error if the request cannot be serialized, the embedded
    /// JavaScript fails, or its response cannot be deserialized.
    pub fn fix(&mut self, text: &str, file_path: &str) -> Result<FixResult> {
        self.invoke("fix", &LintRequest { text, file_path })
    }

    /// Formats lint results with one of the embedded textlint formatters.
    ///
    /// Supported formatter names are `stylish`, `compact`, `json`,
    /// `checkstyle`, `junit`, and `tap`.
    ///
    /// # Errors
    ///
    /// Returns an error if `formatter_name` is unsupported, the embedded
    /// JavaScript fails, or its response cannot be deserialized.
    pub fn format_lint_results(
        &mut self,
        results: &[LintResult],
        formatter_name: &str,
    ) -> Result<String> {
        self.invoke(
            "format",
            &FormatRequest {
                formatter_name,
                results,
            },
        )
    }

    /// Formats fix results with one of the embedded textlint formatters.
    ///
    /// Supported formatter names are `stylish`, `compact`, `json`,
    /// `checkstyle`, `junit`, and `tap`.
    ///
    /// # Errors
    ///
    /// Returns an error if `formatter_name` is unsupported, the embedded
    /// JavaScript fails, or its response cannot be deserialized.
    pub fn format_fix_results(
        &mut self,
        results: &[FixResult],
        formatter_name: &str,
    ) -> Result<String> {
        self.invoke(
            "format",
            &FormatRequest {
                formatter_name,
                results,
            },
        )
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
                "# 1. “見出し” 😀\n\n*強調*\n\n---\n\n## 次\n\n1. 箇条書き\n\n特殊　空白\n\n無効な制御文字\u{b}\n\n革命的な技術です。\n\nこれは見ることができないわけではない。\n",
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
        assert!(ids.contains(&"@textlint-rule/no-invalid-control-character"));
        assert!(ids.contains(&"ja-technical-writing/no-double-negative-ja"));

        let second_result = textlint
            .lint("試したが失敗したが、再試行した。", "second.md")
            .unwrap();
        assert!(second_result.messages.iter().any(|message| {
            message.rule_id == "ja-technical-writing/no-doubled-conjunctive-particle-ga"
        }));
    }

    #[test]
    fn fixes_text_and_formats_results() {
        let mut textlint = Textlint::new().unwrap();
        let fixed = textlint.fix("特殊　空白", "sample.md").unwrap();

        assert_eq!(fixed.output, "特殊 空白");
        assert!(
            fixed
                .applying_messages
                .iter()
                .any(|message| message.rule_id == "@0x6b/normalize-whitespaces")
        );

        let json = textlint.format_fix_results(&[fixed], "json").unwrap();
        assert!(json.contains("\"output\":\"特殊 空白\""));

        let lint = textlint.lint("本文 😀", "sample.md").unwrap();
        let stylish = textlint.format_lint_results(&[lint], "stylish").unwrap();
        assert!(stylish.contains("@0x6b/no-emoji"));
    }

    #[test]
    fn embeds_third_party_notices() {
        let notices = third_party_notices();
        assert!(notices.contains("kuromoji 0.1.2"));
        assert!(notices.contains("@textlint/kernel 15.8.0"));
        assert!(notices.contains("mecab-ipadic-2.7.0-20070801"));
    }
}
