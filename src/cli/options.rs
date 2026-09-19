use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(
    version,
    disable_version_flag = true,
    override_usage = "textlint-v8 [OPTIONS] [FILE|DIR|GLOB]..."
)]
pub(super) struct Args {
    #[arg(short = 'v', long, action = clap::ArgAction::Version)]
    pub(super) version: Option<bool>,
    #[arg(long, alias = "third-party-licenses", conflicts_with = "paths")]
    pub(super) licenses: bool,
    #[arg(long)]
    pub(super) fix: bool,
    #[arg(long)]
    pub(super) dry_run: bool,
    #[arg(short = 'o', long, value_name = "path")]
    pub(super) output_file: Option<PathBuf>,
    #[arg(long)]
    pub(super) quiet: bool,
    #[arg(long)]
    pub(super) experimental: bool,
    #[arg(long, action = clap::ArgAction::SetTrue, overrides_with = "no_color")]
    pub(super) color: bool,
    #[arg(long = "no-color", action = clap::ArgAction::SetTrue, overrides_with = "color")]
    pub(super) no_color: bool,
    #[arg(long, value_name = "path")]
    pub(super) ignore_path: Option<PathBuf>,
    #[arg(long)]
    pub(super) stdin: bool,
    #[arg(long, value_name = "filename")]
    pub(super) stdin_filename: Option<PathBuf>,
    #[arg(
        short = 'f',
        long = "format",
        alias = "formatter",
        default_value = "stylish",
        value_name = "name",
        help = "Use a built-in textlint formatter"
    )]
    pub(super) formatter: String,
    pub(super) paths: Vec<PathBuf>,
}
