use std::{
    fs::read_to_string,
    io::{Read as _, Write as _, stdin, stdout},
    path::PathBuf,
};

use anyhow::{Context as _, Result};
use clap::Parser;
use serde_json::to_writer_pretty;
use textlint_v8::{Textlint, third_party_notices};

#[derive(Parser)]
struct Args {
    #[arg(long, alias = "third-party-licenses", conflicts_with = "paths")]
    licenses: bool,
    paths: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let Args { licenses, paths } = Args::parse();
    if licenses {
        stdout().write_all(third_party_notices().as_bytes())?;
        return Ok(());
    }

    let mut textlint = Textlint::new()?;
    let results = if paths.is_empty() {
        let mut text = String::new();
        stdin().read_to_string(&mut text)?;
        vec![textlint.lint(&text, "stdin.md")?]
    } else {
        paths
            .into_iter()
            .map(|path| -> Result<_> {
                let text = read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.to_string_lossy()))?;
                Ok(textlint.lint(&text, &path.to_string_lossy())?)
            })
            .collect::<Result<Vec<_>>>()?
    };

    let mut stdout = stdout().lock();
    if let [result] = results.as_slice() {
        to_writer_pretty(&mut stdout, result)?;
    } else {
        to_writer_pretty(&mut stdout, &results)?;
    }
    writeln!(stdout)?;
    Ok(())
}
