use std::{
    fs::{read_to_string, write},
    io::{Read as _, Write as _, stdin, stdout},
    path::PathBuf,
};

use anyhow::{Context as _, Result};
use clap::Parser;
use textlint_v8::{Textlint, third_party_notices};

#[derive(Parser)]
struct Args {
    #[arg(long, alias = "third-party-licenses", conflicts_with = "paths")]
    licenses: bool,
    #[arg(long)]
    fix: bool,
    #[arg(
        long,
        default_value = "stylish",
        value_name = "name",
        help = "Formatter: stylish, compact, json, checkstyle, junit, or tap"
    )]
    formatter: String,
    paths: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let Args {
        licenses,
        fix,
        formatter,
        paths,
    } = Args::parse();
    if licenses {
        stdout().write_all(third_party_notices().as_bytes())?;
        return Ok(());
    }

    let mut textlint = Textlint::new()?;
    if paths.is_empty() {
        let mut text = String::new();
        stdin().read_to_string(&mut text)?;
        if fix {
            stdout().write_all(textlint.fix(&text, "stdin.md")?.output.as_bytes())?;
        } else {
            let results = [textlint.lint(&text, "stdin.md")?];
            writeln!(
                stdout(),
                "{}",
                textlint.format_lint_results(&results, &formatter)?
            )?;
        }
        return Ok(());
    }

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
        writeln!(
            stdout(),
            "{}",
            textlint.format_fix_results(&results, &formatter)?
        )?;
    } else {
        let results = paths
            .into_iter()
            .map(|path| -> Result<_> {
                let text = read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.to_string_lossy()))?;
                Ok(textlint.lint(&text, &path.to_string_lossy())?)
            })
            .collect::<Result<Vec<_>>>()?;
        writeln!(
            stdout(),
            "{}",
            textlint.format_lint_results(&results, &formatter)?
        )?;
    }

    Ok(())
}
