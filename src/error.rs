use thiserror::Error;

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
