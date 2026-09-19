use std::{cell::RefCell, fmt::Display, fs::read_to_string, path::PathBuf, rc::Rc};

use anyhow::{Context as _, Error, Result};
use rmcp::{
    ServerHandler, ServiceExt as _,
    handler::server::{tool::schema_for_output, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use serde_json::to_value;
use textlint_v8::{FixResult, LintMessage, LintResult, Textlint};
use tokio::{runtime::Builder, task::LocalSet};

use super::targets::resolve;

#[derive(Clone)]
struct Server {
    textlint: Rc<RefCell<Textlint>>,
}

/// Markdown targets to process with the embedded textlint rules.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct FileInput {
    /// One or more file paths, directories, or glob patterns.
    #[schemars(length(min = 1), inner(length(min = 1)))]
    file_paths: Vec<PathBuf>,
}

/// Markdown text to process with the embedded textlint rules.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct TextInput {
    /// The complete Markdown source text.
    #[schemars(length(min = 1))]
    text: String,
    /// The filename used to identify the text in diagnostics, such as `document.md`.
    #[schemars(length(min = 1))]
    stdin_filename: String,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolOutput {
    files: Vec<FileOutput>,
    summary: Summary,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileOutput {
    path: String,
    diagnostics: Vec<Diagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    applied_fix_count: Option<usize>,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
struct Diagnostic {
    rule_id: String,
    severity: Severity,
    message: String,
    line: usize,
    column: usize,
    end_line: usize,
    end_column: usize,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    file_count: usize,
    diagnostic_count: usize,
    error_count: usize,
    warning_count: usize,
    info_count: usize,
    applied_fix_count: usize,
}

#[tool_router]
impl Server {
    fn new() -> textlint_v8::Result<Self> {
        Ok(Self { textlint: Rc::new(RefCell::new(Textlint::new()?)) })
    }

    #[tool(
        name = "lintFile",
        description = "Lint Markdown files, directories, or glob patterns with the embedded textlint rules",
        output_schema = schema_for_output::<ToolOutput>(),
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    fn lint_file(
        &self,
        Parameters(FileInput { file_paths }): Parameters<FileInput>,
    ) -> Result<CallToolResult, String> {
        let documents = read_files(&file_paths).map_err(display_error)?;
        let mut textlint = self.textlint.borrow_mut();
        let results = documents
            .iter()
            .map(|(path, text)| textlint.lint(text, path))
            .collect::<textlint_v8::Result<Vec<_>>>()
            .map_err(display_error)?;
        Ok(lint_result(results))
    }

    #[tool(
        name = "lintText",
        description = "Lint Markdown text with the embedded textlint rules",
        output_schema = schema_for_output::<ToolOutput>(),
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    fn lint_text(
        &self,
        Parameters(TextInput { text, stdin_filename }): Parameters<TextInput>,
    ) -> Result<CallToolResult, String> {
        let result = self
            .textlint
            .borrow_mut()
            .lint(&text, &stdin_filename)
            .map_err(display_error)?;
        Ok(lint_result(vec![result]))
    }

    #[tool(
        name = "getLintFixedFileContent",
        description = "Return fixed Markdown file content without modifying files on disk",
        output_schema = schema_for_output::<ToolOutput>(),
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    fn fix_file(
        &self,
        Parameters(FileInput { file_paths }): Parameters<FileInput>,
    ) -> Result<CallToolResult, String> {
        let documents = read_files(&file_paths).map_err(display_error)?;
        let mut textlint = self.textlint.borrow_mut();
        let results = documents
            .iter()
            .map(|(path, text)| textlint.fix(text, path))
            .collect::<textlint_v8::Result<Vec<_>>>()
            .map_err(display_error)?;
        Ok(fix_result(results))
    }

    #[tool(
        name = "getLintFixedTextContent",
        description = "Return fixed Markdown text using the embedded textlint rules",
        output_schema = schema_for_output::<ToolOutput>(),
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    fn fix_text(
        &self,
        Parameters(TextInput { text, stdin_filename }): Parameters<TextInput>,
    ) -> Result<CallToolResult, String> {
        let result = self
            .textlint
            .borrow_mut()
            .fix(&text, &stdin_filename)
            .map_err(display_error)?;
        Ok(fix_result(vec![result]))
    }
}

#[tool_handler]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Lint and fix Markdown using textlint-v8's fixed embedded rules. Fix tools never modify files on disk.",
            )
    }
}

pub(super) fn run() -> Result<u8> {
    let runtime = Builder::new_current_thread().enable_time().build()?;
    LocalSet::new().block_on(&runtime, async {
        Server::new()?.serve(stdio()).await?.waiting().await?;
        Ok::<_, Error>(())
    })?;
    Ok(0)
}

fn read_files(file_paths: &[PathBuf]) -> Result<Vec<(String, String)>> {
    resolve(file_paths, None)?
        .into_iter()
        .map(|path| {
            let text = read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            Ok((path.to_string_lossy().into_owned(), text))
        })
        .collect()
}

fn lint_result(results: Vec<LintResult>) -> CallToolResult {
    let files = results
        .into_iter()
        .map(|result| FileOutput {
            path: result.file_path,
            diagnostics: diagnostics(result.messages),
            output: None,
            applied_fix_count: None,
        })
        .collect();
    tool_result(files)
}

fn fix_result(results: Vec<FixResult>) -> CallToolResult {
    let files = results
        .into_iter()
        .map(|result| FileOutput {
            path: result.file_path,
            diagnostics: diagnostics(result.remaining_messages),
            output: Some(result.output),
            applied_fix_count: Some(result.applying_messages.len()),
        })
        .collect();
    tool_result(files)
}

fn diagnostics(messages: Vec<LintMessage>) -> Vec<Diagnostic> {
    messages
        .into_iter()
        .map(|message| Diagnostic {
            rule_id: message.rule_id,
            severity: match message.severity {
                2 => Severity::Error,
                1 => Severity::Warning,
                _ => Severity::Info,
            },
            message: message.message,
            line: message.line,
            column: message.column,
            end_line: message.loc.end.line,
            end_column: message.loc.end.column,
        })
        .collect()
}

fn tool_result(files: Vec<FileOutput>) -> CallToolResult {
    let summary = summarize(&files);
    let content = format!(
        "Processed {} file(s): {} error(s), {} warning(s), {} informational diagnostic(s), {} fix(es) applied.",
        summary.file_count,
        summary.error_count,
        summary.warning_count,
        summary.info_count,
        summary.applied_fix_count,
    );
    let output = ToolOutput { files, summary };
    let mut result =
        CallToolResult::structured(to_value(output).expect("tool output is serializable"));
    result.content = vec![ContentBlock::text(content)];
    result
}

fn summarize(files: &[FileOutput]) -> Summary {
    let mut summary = Summary {
        file_count: files.len(),
        diagnostic_count: 0,
        error_count: 0,
        warning_count: 0,
        info_count: 0,
        applied_fix_count: 0,
    };
    for file in files {
        summary.diagnostic_count += file.diagnostics.len();
        summary.applied_fix_count += file.applied_fix_count.unwrap_or_default();
        for diagnostic in &file.diagnostics {
            match diagnostic.severity {
                Severity::Error => summary.error_count += 1,
                Severity::Warning => summary.warning_count += 1,
                Severity::Info => summary.info_count += 1,
            }
        }
    }
    summary
}

fn display_error(error: impl Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use std::{
        env::temp_dir,
        fs::{read_to_string, remove_file, write},
        time::SystemTime,
    };

    use super::*;

    #[test]
    fn returns_structured_results_without_modifying_fixed_files() {
        let server = Server::new().unwrap();
        let lint = server
            .lint_text(Parameters(TextInput {
                text: "本文 😀。".to_owned(),
                stdin_filename: "sample.md".to_owned(),
            }))
            .unwrap();
        let lint = lint.structured_content.unwrap();
        assert_eq!(lint["files"][0]["path"], "sample.md");
        assert!(lint["summary"]["diagnosticCount"].as_u64().unwrap() > 0);

        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = temp_dir().join(format!("textlint-v8-mcp-{unique}.md"));
        let original = "特殊　空白。";
        write(&path, original).unwrap();

        let fixed = server
            .fix_file(Parameters(FileInput { file_paths: vec![path.clone()] }))
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(fixed["files"][0]["output"], "特殊 空白。");
        assert_eq!(fixed["summary"]["appliedFixCount"], 1);
        assert_eq!(read_to_string(&path).unwrap(), original);

        remove_file(path).unwrap();
    }
}
