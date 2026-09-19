use std::{
    fs::{create_dir_all, read_to_string, write},
    io,
    io::{Read as _, Write as _, stdout},
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{Context as _, Result, bail};
use clap::{CommandFactory as _, Parser};
use cli::targets::resolve;
use textlint_v8::{FixResult, LintResult, Textlint, third_party_notices};

mod cli;

#[derive(Parser)]
#[command(
    version,
    disable_version_flag = true,
    override_usage = "textlint-v8 [OPTIONS] [FILE|DIR|GLOB]..."
)]
struct Args {
    #[arg(short = 'v', long, action = clap::ArgAction::Version)]
    version: Option<bool>,
    #[arg(long, alias = "third-party-licenses", conflicts_with = "paths")]
    licenses: bool,
    #[arg(long)]
    fix: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(short = 'o', long, value_name = "path")]
    output_file: Option<PathBuf>,
    #[arg(long)]
    quiet: bool,
    #[arg(long)]
    experimental: bool,
    #[arg(long, action = clap::ArgAction::SetTrue, overrides_with = "no_color")]
    color: bool,
    #[arg(long = "no-color", action = clap::ArgAction::SetTrue, overrides_with = "color")]
    no_color: bool,
    #[arg(long, value_name = "path")]
    ignore_path: Option<PathBuf>,
    #[arg(long)]
    stdin: bool,
    #[arg(long, value_name = "filename")]
    stdin_filename: Option<PathBuf>,
    #[arg(
        short = 'f',
        long = "format",
        alias = "formatter",
        default_value = "stylish",
        value_name = "name",
        help = "Use a built-in textlint formatter"
    )]
    formatter: String,
    paths: Vec<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<u8> {
    let args = Args::parse();
    let Args {
        version: _,
        licenses,
        fix,
        dry_run,
        output_file,
        quiet,
        experimental: _,
        color,
        no_color,
        ignore_path,
        stdin,
        stdin_filename,
        formatter,
        paths,
    } = args;
    let color = color || !no_color;
    if licenses {
        stdout().write_all(third_party_notices().as_bytes())?;
        return Ok(0);
    }

    if stdin {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text)?;
        if !text.is_empty() {
            let Some(stdin_filename) = stdin_filename else {
                bail!("Please specify --stdin-filename option")
            };
            let mut textlint = Textlint::new()?;
            let filename = stdin_filename.to_string_lossy();
            if fix {
                let mut results = [textlint.fix(&text, &filename)?];
                let has_errors = fix_has_errors(&results);
                if quiet {
                    retain_errors_in_fix_results(&mut results);
                }
                let output = textlint.format_fix_results_with_color(&results, &formatter, color)?;
                write_output(&output, output_file.as_ref())?;
                return Ok(status(has_errors, output_file.is_some() || dry_run));
            } else {
                let mut results = [textlint.lint(&text, &filename)?];
                let has_errors = lint_has_errors(&results);
                if quiet {
                    retain_errors_in_lint_results(&mut results);
                }
                let output =
                    textlint.format_lint_results_with_color(&results, &formatter, color)?;
                write_output(&output, output_file.as_ref())?;
                return Ok(status(has_errors, output_file.is_some()));
            }
        }
    }

    if paths.is_empty() {
        Args::command().print_help()?;
        return Ok(0);
    }

    let mut textlint = Textlint::new()?;
    let paths = resolve(&paths, ignore_path.as_deref())?;
    if fix {
        let mut results = paths
            .into_iter()
            .map(|path| -> Result<_> {
                let text = read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.to_string_lossy()))?;
                Ok(textlint.fix(&text, &path.to_string_lossy())?)
            })
            .collect::<Result<Vec<_>>>()?;
        let has_errors = fix_has_errors(&results);
        if !dry_run {
            for result in &results {
                write(&result.file_path, &result.output)
                    .with_context(|| format!("failed to write {}", result.file_path))?;
            }
        }
        if quiet {
            retain_errors_in_fix_results(&mut results);
        }
        let output = textlint.format_fix_results_with_color(&results, &formatter, color)?;
        write_output(&output, output_file.as_ref())?;
        Ok(status(has_errors, output_file.is_some() || dry_run))
    } else {
        let mut results = paths
            .into_iter()
            .map(|path| -> Result<_> {
                let text = read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.to_string_lossy()))?;
                Ok(textlint.lint(&text, &path.to_string_lossy())?)
            })
            .collect::<Result<Vec<_>>>()?;
        let has_errors = lint_has_errors(&results);
        if quiet {
            retain_errors_in_lint_results(&mut results);
        }
        let output = textlint.format_lint_results_with_color(&results, &formatter, color)?;
        write_output(&output, output_file.as_ref())?;
        Ok(status(has_errors, output_file.is_some()))
    }
}

fn lint_has_errors(results: &[LintResult]) -> bool {
    results
        .iter()
        .any(|result| result.messages.iter().any(|message| message.severity == 2))
}

fn fix_has_errors(results: &[FixResult]) -> bool {
    results
        .iter()
        .any(|result| result.remaining_messages.iter().any(|message| message.severity == 2))
}

fn retain_errors_in_lint_results(results: &mut [LintResult]) {
    for result in results {
        result.messages.retain(|message| message.severity == 2);
    }
}

fn retain_errors_in_fix_results(results: &mut [FixResult]) {
    for result in results {
        result.messages.retain(|message| message.severity == 2);
        result.applying_messages.retain(|message| message.severity == 2);
        result.remaining_messages.retain(|message| message.severity == 2);
    }
}

fn write_output(output: &str, output_file: Option<&PathBuf>) -> Result<()> {
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
