use std::{
    fmt::Display,
    fs::read_to_string,
    net::SocketAddr,
    path::PathBuf,
    sync::mpsc::{Receiver, Sender, channel},
    thread,
};

use anyhow::{Context as _, Error, Result, bail};
use axum::{Router, http::StatusCode, routing::get, serve};
use rmcp::{
    ServerHandler, ServiceExt as _,
    handler::server::{tool::schema_for_output, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
    transport::{
        stdio,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use serde::{Deserialize, Serialize};
use serde_json::to_value;
use textlint_v8::{FixResult, LintMessage, LintResult, Textlint};
use tokio::{net::TcpListener, runtime::Builder, select, sync::oneshot};
use tokio_util::sync::CancellationToken;

use super::targets::resolve;

#[derive(Clone)]
struct Server {
    worker: Sender<WorkerRequest>,
}

type WorkerResponse = Result<CallToolResult, String>;

enum WorkerRequest {
    LintFile(FileInput, oneshot::Sender<WorkerResponse>),
    LintText(TextInput, oneshot::Sender<WorkerResponse>),
    FixFile(FileInput, oneshot::Sender<WorkerResponse>),
    FixText(TextInput, oneshot::Sender<WorkerResponse>),
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
    fn new() -> Result<Self> {
        let (worker, requests) = channel();
        let (ready, initialized) = channel();
        thread::Builder::new()
            .name("textlint-v8-mcp".to_owned())
            .spawn(move || run_worker(requests, &ready))
            .context("spawn textlint MCP worker")?;
        initialized
            .recv()
            .context("textlint MCP worker stopped during initialization")?
            .map_err(Error::msg)?;
        Ok(Self { worker })
    }

    async fn request(
        &self,
        request: impl FnOnce(oneshot::Sender<WorkerResponse>) -> WorkerRequest,
    ) -> WorkerResponse {
        let (response, receiver) = oneshot::channel();
        self.worker
            .send(request(response))
            .map_err(|_| "textlint MCP worker is unavailable".to_owned())?;
        receiver
            .await
            .map_err(|_| "textlint MCP worker stopped before responding".to_owned())?
    }

    #[tool(
        name = "lintFile",
        description = "Lint Markdown files, directories, or glob patterns with the embedded textlint rules",
        output_schema = schema_for_output::<ToolOutput>(),
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn lint_file(
        &self,
        Parameters(input): Parameters<FileInput>,
    ) -> Result<CallToolResult, String> {
        self.request(|response| WorkerRequest::LintFile(input, response))
            .await
    }

    #[tool(
        name = "lintText",
        description = "Lint Markdown text with the embedded textlint rules",
        output_schema = schema_for_output::<ToolOutput>(),
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn lint_text(
        &self,
        Parameters(input): Parameters<TextInput>,
    ) -> Result<CallToolResult, String> {
        self.request(|response| WorkerRequest::LintText(input, response))
            .await
    }

    #[tool(
        name = "getLintFixedFileContent",
        description = "Return fixed Markdown file content without modifying files on disk",
        output_schema = schema_for_output::<ToolOutput>(),
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn fix_file(
        &self,
        Parameters(input): Parameters<FileInput>,
    ) -> Result<CallToolResult, String> {
        self.request(|response| WorkerRequest::FixFile(input, response)).await
    }

    #[tool(
        name = "getLintFixedTextContent",
        description = "Return fixed Markdown text using the embedded textlint rules",
        output_schema = schema_for_output::<ToolOutput>(),
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn fix_text(
        &self,
        Parameters(input): Parameters<TextInput>,
    ) -> Result<CallToolResult, String> {
        self.request(|response| WorkerRequest::FixText(input, response)).await
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
    runtime.block_on(async {
        Server::new()?.serve(stdio()).await?.waiting().await?;
        Ok::<_, Error>(())
    })?;
    Ok(0)
}

pub(super) fn run_http(address: SocketAddr, allowed_hosts: Vec<String>) -> Result<u8> {
    if !address.ip().is_loopback() && allowed_hosts.is_empty() {
        bail!("--mcp-http-allowed-host is required for a non-loopback MCP HTTP listener");
    }

    let runtime = Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async move {
        let server = Server::new()?;
        let cancellation = CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default()
            .enforce_origin_validation()
            .with_cancellation_token(cancellation.child_token());
        if !allowed_hosts.is_empty() {
            config = config.with_allowed_hosts(allowed_hosts);
        }
        let service = StreamableHttpService::new(
            move || Ok(server.clone()),
            LocalSessionManager::default().into(),
            config,
        );
        let router = Router::new()
            .route("/healthz", get(|| async { StatusCode::OK }))
            .nest_service("/mcp", service);
        let listener = TcpListener::bind(address)
            .await
            .with_context(|| format!("bind MCP HTTP listener to {address}"))?;
        eprintln!("textlint-v8 MCP server listening on http://{address}/mcp");
        serve(listener, router)
            .with_graceful_shutdown(shutdown(cancellation))
            .await
            .context("serve MCP HTTP")?;
        Ok::<_, Error>(())
    })?;
    Ok(0)
}

async fn shutdown(cancellation: CancellationToken) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        use tokio::signal::ctrl_c;

        let _ = ctrl_c().await;
    }
    cancellation.cancel();
}

fn run_worker(requests: Receiver<WorkerRequest>, ready: &Sender<Result<(), String>>) {
    let mut textlint = match Textlint::new() {
        Ok(textlint) => {
            let _ = ready.send(Ok(()));
            textlint
        }
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    for request in requests {
        match request {
            WorkerRequest::LintFile(FileInput { file_paths }, response) => {
                let result = read_files(&file_paths)
                    .and_then(|documents| {
                        documents
                            .iter()
                            .map(|(path, text)| textlint.lint(text, path).map_err(Error::from))
                            .collect::<Result<Vec<_>>>()
                    })
                    .map(lint_result)
                    .map_err(display_error);
                let _ = response.send(result);
            }
            WorkerRequest::LintText(TextInput { text, stdin_filename }, response) => {
                let result = textlint
                    .lint(&text, &stdin_filename)
                    .map(|result| lint_result(vec![result]))
                    .map_err(display_error);
                let _ = response.send(result);
            }
            WorkerRequest::FixFile(FileInput { file_paths }, response) => {
                let result = read_files(&file_paths)
                    .and_then(|documents| {
                        documents
                            .iter()
                            .map(|(path, text)| textlint.fix(text, path).map_err(Error::from))
                            .collect::<Result<Vec<_>>>()
                    })
                    .map(fix_result)
                    .map_err(display_error);
                let _ = response.send(result);
            }
            WorkerRequest::FixText(TextInput { text, stdin_filename }, response) => {
                let result = textlint
                    .fix(&text, &stdin_filename)
                    .map(|result| fix_result(vec![result]))
                    .map_err(display_error);
                let _ = response.send(result);
            }
        }
    }
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
        Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let server = Server::new().unwrap();
                let lint = server
                    .lint_text(Parameters(TextInput {
                        text: "本文 😀。".to_owned(),
                        stdin_filename: "sample.md".to_owned(),
                    }))
                    .await
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
                    .await
                    .unwrap()
                    .structured_content
                    .unwrap();
                assert_eq!(fixed["files"][0]["output"], "特殊 空白。");
                assert_eq!(fixed["summary"]["appliedFixCount"], 1);
                assert_eq!(read_to_string(&path).unwrap(), original);

                remove_file(path).unwrap();
            });
    }
}
