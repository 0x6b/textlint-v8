use std::{
    env::args_os,
    fs::read_to_string,
    io::{Read as _, Write as _, stdin, stdout},
};

use anyhow::{Context as _, Result};
use serde_json::to_string_pretty;
use textlint_v8::{Textlint, third_party_notices};

fn main() -> Result<()> {
    let paths: Vec<_> = args_os().skip(1).collect();
    if paths.len() == 1
        && paths[0]
            .to_str()
            .is_some_and(|arg| matches!(arg, "--licenses" | "--third-party-licenses"))
    {
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
                let file_path = path.to_string_lossy().into_owned();
                Ok(textlint.lint(&text, &file_path)?)
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
