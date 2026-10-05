//! Generate config.toml and config.schema.json into examples/.
//!
//! Run with: `cargo run -p skdlr-core --example generate_config`
//! or via `just generate-config`.

// CLI-style generator reports progress on stdout.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;

use anyhow::Context as _;
use skdlr_core::{APP_NAME, REPO_URL, write_generated_files};

fn main() -> anyhow::Result<()> {
    // Find the workspace root (where examples/ lives).
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let crate_root = PathBuf::from(&manifest_dir);
    let workspace_root = crate_root
        .parent() // crates/
        .and_then(|p| p.parent()) // workspace root
        .context("finding workspace root from crate path")?;

    let examples_dir = workspace_root.join("examples");

    println!("Generating config files to {}...", examples_dir.display());
    write_generated_files(&examples_dir, REPO_URL)?;
    println!("Done! Generated:");
    println!("  - {}/config.schema.json", examples_dir.display());
    println!("  - {}/config.toml", examples_dir.display());

    Ok(())
}
