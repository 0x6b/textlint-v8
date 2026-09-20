use std::{
    fs::{create_dir_all, read_to_string, write},
    io::{Read as _, Write as _, stdin, stdout},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use clap::{CommandFactory as _, Parser as _};
use textlint_v8::{FixResult, LintMessage, LintResult, Textlint, third_party_notices};

use super::{mcp, mcp::run_http, options::Args, targets::resolve};

struct Document {
    text: String,
    file_path: String,
    source_path: Option<PathBuf>,
}

pub(crate) fn run() -> Result<u8> {
    let args = Args::parse();
    if args.mcp {
        return mcp::run();
    }
    if let Some(address) = args.mcp_http {
        return run_http(address, args.mcp_http_allowed_host);
    }
    if args.licenses {
        stdout().write_all(third_party_notices().as_bytes())?;
        return Ok(0);
    }

    let Some(documents) = collect_documents(&args)? else {
        Args::command().print_help()?;
        return Ok(0);
    };
    let mut textlint = Textlint::new()?;
    if args.fix {
        run_fixer(&args, &documents, &mut textlint)
    } else {
        run_linter(&args, &documents, &mut textlint)
    }
}

fn collect_documents(args: &Args) -> Result<Option<Vec<Document>>> {
    if args.stdin {
        let mut text = String::new();
        stdin().read_to_string(&mut text)?;
        if !text.is_empty() {
            let Some(file_path) = &args.stdin_filename else {
                bail!("Please specify --stdin-filename option")
            };
            return Ok(Some(vec![Document {
                text,
                file_path: file_path.to_string_lossy().into_owned(),
                source_path: None,
            }]));
        }
    }

    if args.paths.is_empty() {
        return Ok(None);
    }
    resolve(&args.paths, args.ignore_path.as_deref())?
        .into_iter()
        .map(|path| {
            let text = read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            Ok(Document {
                text,
                file_path: path.to_string_lossy().into_owned(),
                source_path: Some(path),
            })
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn run_linter(args: &Args, documents: &[Document], textlint: &mut Textlint) -> Result<u8> {
    let mut results = documents
        .iter()
        .map(|document| textlint.lint(&document.text, &document.file_path))
        .collect::<textlint_v8::Result<Vec<_>>>()?;
    let has_errors = lint_has_errors(&results);
    if args.quiet {
        for result in &mut results {
            result.messages.retain(is_error);
        }
    }
    let output = textlint.format_lint_results_with_color(
        &results,
        &args.formatter,
        args.color || !args.no_color,
    )?;
    write_output(&output, args.output_file.as_deref())?;
    Ok(status(has_errors, args.output_file.is_some()))
}

fn run_fixer(args: &Args, documents: &[Document], textlint: &mut Textlint) -> Result<u8> {
    let mut results = documents
        .iter()
        .map(|document| textlint.fix(&document.text, &document.file_path))
        .collect::<textlint_v8::Result<Vec<_>>>()?;
    let has_errors = fix_has_errors(&results);
    if !args.dry_run {
        for (document, result) in documents.iter().zip(&results) {
            if let Some(path) = &document.source_path {
                write(path, &result.output)
                    .with_context(|| format!("failed to write {}", path.display()))?;
            }
        }
    }
    if args.quiet {
        for result in &mut results {
            result.messages.retain(is_error);
            result.applying_messages.retain(is_error);
            result.remaining_messages.retain(is_error);
        }
    }
    let output = textlint.format_fix_results_with_color(
        &results,
        &args.formatter,
        args.color || !args.no_color,
    )?;
    write_output(&output, args.output_file.as_deref())?;
    Ok(status(has_errors, args.output_file.is_some() || args.dry_run))
}

fn lint_has_errors(results: &[LintResult]) -> bool {
    results.iter().any(|result| result.messages.iter().any(is_error))
}

fn fix_has_errors(results: &[FixResult]) -> bool {
    results
        .iter()
        .any(|result| result.remaining_messages.iter().any(is_error))
}

fn is_error(message: &LintMessage) -> bool {
    message.severity == 2
}

fn write_output(output: &str, output_file: Option<&Path>) -> Result<()> {
    if let Some(path) = output_file {
        if let Some(parent) = path.parent().filter(|parent| *parent != Path::new("")) {
            create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        write(path, output).with_context(|| format!("failed to write {}", path.display()))?;
    } else {
        stdout().write_all(output.as_bytes())?;
    }
    Ok(())
}

fn status(has_errors: bool, force_success: bool) -> u8 {
    u8::from(has_errors && !force_success)
}
