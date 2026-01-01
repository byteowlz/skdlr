//! skdlr-mcp - MCP server for AI agent integration.
//!
//! This is a placeholder implementation. Full MCP implementation is tracked in:
//! https://github.com/byteowlz/skdlr/issues/skdlr-d4w

use std::io::{self, Write};

use anyhow::Result;

fn main() {
    if let Err(err) = run() {
        let _ = writeln!(io::stderr(), "error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    eprintln!("skdlr-mcp: MCP server not yet implemented");
    eprintln!("See: https://github.com/byteowlz/skdlr/issues/skdlr-d4w");
    Ok(())
}
