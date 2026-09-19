use std::{
    fs::{read_to_string, write},
    io,
    io::{Read as _, Write as _, stdout},
    path::PathBuf,
};

use anyhow::{Context as _, Result, bail};
use clap::{CommandFactory as _, Parser};
use cli::targets::resolve;
use textlint_v8::{Textlint, third_party_notices};

mod cli;

#[derive(Parser)]
#[command(
    version,
    disable_version_flag = true,
    override_usage = "textlint-v8 [OPTIONS] [FILE|DIR|GLOB]..."
)]
struct Args {
    #[arg(short = 'v', long, action = clap::ArgAction::Version)]
    version: bool,
    #[arg(long, alias = "third-party-licenses", conflicts_with = "paths")]
    licenses: bool,
    #[arg(long)]
    fix: bool,
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
        help = "Formatter: stylish, compact, json, checkstyle, junit, or tap"
    )]
    formatter: String,
    paths: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let Args {
        version: _,
        licenses,
        fix,
        ignore_path,
        stdin,
        stdin_filename,
        formatter,
        paths,
    } = args;
    if licenses {
        stdout().write_all(third_party_notices().as_bytes())?;
        return Ok(());
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
                stdout().write_all(textlint.fix(&text, &filename)?.output.as_bytes())?;
            } else {
                let results = [textlint.lint(&text, &filename)?];
                stdout()
                    .write_all(textlint.format_lint_results(&results, &formatter)?.as_bytes())?;
            }
            return Ok(());
        }
    }

    if paths.is_empty() {
        Args::command().print_help()?;
        return Ok(());
    }

    let mut textlint = Textlint::new()?;
    let paths = resolve(&paths, ignore_path.as_deref())?;
    if fix {
        let results = paths
            .into_iter()
            .map(|path| -> Result<_> {
                let text = read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.to_string_lossy()))?;
                Ok(textlint.fix(&text, &path.to_string_lossy())?)
            })
            .collect::<Result<Vec<_>>>()?;
        for result in &results {
            write(&result.file_path, &result.output)
                .with_context(|| format!("failed to write {}", result.file_path))?;
        }
        writeln!(stdout(), "{}", textlint.format_fix_results(&results, &formatter)?)?;
    } else {
        let results = paths
            .into_iter()
            .map(|path| -> Result<_> {
                let text = read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.to_string_lossy()))?;
                Ok(textlint.lint(&text, &path.to_string_lossy())?)
            })
            .collect::<Result<Vec<_>>>()?;
        writeln!(stdout(), "{}", textlint.format_lint_results(&results, &formatter)?)?;
    }

    Ok(())
}
