use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "rginx",
    version,
    about = "A Rust edge reverse proxy for small and medium deployments"
)]
pub struct Cli {
    #[arg(short, long, global = true, default_value = "configs/rginx.ron")]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Copy, Subcommand)]
pub enum Command {
    Check,
}
