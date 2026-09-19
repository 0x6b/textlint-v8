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

mod error;
mod notices;
mod runtime;
mod types;

pub use error::{Error, Result};
pub use notices::third_party_notices;
pub use runtime::Textlint;
pub use types::{FixResult, LintLocation, LintMessage, LintPosition, LintResult};
