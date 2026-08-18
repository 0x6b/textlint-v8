use std::{env, fs, io::Read as _};

use anyhow::{Context as _, Result};
use textlint_v8::Textlint;

fn main() -> Result<()> {
    let paths: Vec<_> = env::args_os().skip(1).collect();
    let textlint = Textlint::new()?;
    let results = if paths.is_empty() {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        vec![textlint.lint(&text, "stdin.md")?]
    } else {
        paths
            .into_iter()
            .map(|path| {
                let text = fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.to_string_lossy()))?;
                let file_path = path.to_string_lossy().into_owned();
                textlint.lint(&text, &file_path)
            })
            .collect::<Result<Vec<_>>>()?
    };

    if let [result] = results.as_slice() {
        println!("{}", serde_json::to_string_pretty(result)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&results)?);
    }
    Ok(())
}
