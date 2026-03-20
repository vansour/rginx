use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "rginx", about = "A Rust-first reverse proxy scaffold")]
pub struct Cli {
    #[arg(short, long, default_value = "configs/rginx.ron")]
    pub config: PathBuf,
}
