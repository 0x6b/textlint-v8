use anyhow::Result;
use build::run;
#[path = "build/mod.rs"]
mod build;

fn main() -> Result<()> {
    run()
}
