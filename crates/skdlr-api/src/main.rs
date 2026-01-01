//! skdlr-api - HTTP API server for octo integration.
//!
//! This is a placeholder implementation. Full API implementation is tracked in:
//! https://github.com/byteowlz/skdlr/issues/skdlr-axe

use std::io::{self, Write};
use std::net::SocketAddr;

use anyhow::Result;
use axum::{Json, Router, routing::get};
use clap::{Args, Parser};
use log::info;
use serde::Serialize;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

fn main() {
    if let Err(err) = try_main() {
        let _ = writeln!(io::stderr(), "error: {err:#}");
        std::process::exit(1);
    }
}

#[tokio::main]
async fn try_main() -> Result<()> {
    env_logger::init();

    let cli = Cli::parse();

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([127, 0, 0, 1], cli.common.port));
    info!("Starting API server on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Debug, Parser)]
#[command(author, version, about = "HTTP API server for skdlr")]
struct Cli {
    #[command(flatten)]
    common: CommonOpts,
}

#[derive(Debug, Clone, Args)]
struct CommonOpts {
    /// Port to listen on
    #[arg(short, long, default_value = "3000")]
    port: u16,
}

#[derive(Serialize)]
struct RootResponse {
    name: &'static str,
    version: &'static str,
    note: &'static str,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn root() -> Json<RootResponse> {
    Json(RootResponse {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        note: "Full API not yet implemented. See https://github.com/byteowlz/skdlr/issues/skdlr-axe",
    })
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}
