//! `aka` CLI.

use anyhow::Result;

fn main() -> Result<()> {
    aka_cli::commands::run_from_env()
}
