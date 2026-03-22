use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "rginx", version, about = "A Rust-first reverse proxy scaffold")]
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
