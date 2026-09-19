use serde::{Deserialize, Serialize};

/// The diagnostics produced by linting one document.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LintResult {
    /// The path supplied to [`crate::Textlint::lint`].
    pub file_path: String,
    /// The diagnostics reported for the document.
    pub messages: Vec<LintMessage>,
}

/// The output and diagnostics produced by fixing one document.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FixResult {
    /// The path supplied to [`crate::Textlint::fix`].
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

/// A diagnostic reported by a textlint rule.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
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
