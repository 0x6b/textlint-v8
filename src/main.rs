use std::process::ExitCode;

use cli::run;

mod cli;

fn main() -> ExitCode {
    match run() {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::from(2)
        }
    }
}
