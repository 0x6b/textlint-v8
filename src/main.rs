use std::{
    env::args_os,
    fs::read_to_string,
    io::{Read as _, stdin},
};

use anyhow::{Context as _, Result};
use serde_json::to_string_pretty;
use textlint_v8::Textlint;

fn main() -> Result<()> {
    let paths: Vec<_> = args_os().skip(1).collect();
    let textlint = Textlint::new()?;
    let results = if paths.is_empty() {
        let mut text = String::new();
        stdin().read_to_string(&mut text)?;
        vec![textlint.lint(&text, "stdin.md")?]
    } else {
        paths
            .into_iter()
            .map(|path| {
                let text = read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.to_string_lossy()))?;
                let file_path = path.to_string_lossy().into_owned();
                textlint.lint(&text, &file_path)
            })
            .collect::<Result<Vec<_>>>()?
    };

    if let [result] = results.as_slice() {
        println!("{}", to_string_pretty(result)?);
    } else {
        println!("{}", to_string_pretty(&results)?);
    }
    Ok(())
}
