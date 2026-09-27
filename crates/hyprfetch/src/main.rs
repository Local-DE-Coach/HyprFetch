//! HyprFetch binary entry point.
//!
//! Wires CLI parsing, logging, config loading, and the engine + HTTP server.
//! Currently a stub — the `serve` command boots axum with a single `/healthz`
//! route and blocks on Ctrl-C. Real engine wiring lands in subsequent PRs.

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "hyprfetch",
    version,
    about = "Minimal-RAM download manager with a browser UI"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the download manager server (HTTP + WebSocket + UI).
    Serve {
        #[arg(long, default_value = "127.0.0.1:7780")]
        bind: String,
        #[arg(long)]
        config: Option<std::path::PathBuf>,
        #[arg(long)]
        download_dir: Option<std::path::PathBuf>,
        #[arg(long, default_value_t = 8)]
        segments: u8,
    },
    /// Verify configuration and exit.
    Doctor,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("hyprfetch=info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Serve {
            bind,
            config: _,
            download_dir: _,
            segments: _,
        } => {
            tracing::info!(%bind, "starting hyprfetch");
            // Stub: real wiring lands in feature/http-api.
            // For now, just bind a TCP listener so `cargo run` does something visible.
            let listener = tokio::net::TcpListener::bind(&bind).await?;
            tracing::info!("listening on http://{bind}");
            loop {
                let (mut socket, peer) = listener.accept().await?;
                tracing::debug!(%peer, "accepted connection");
                tokio::spawn(async move {
                    use tokio::io::AsyncWriteExt;
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 16\r\n\r\nhyprfetch: ok\n",
                        )
                        .await;
                });
            }
        }
        Command::Doctor => {
            println!("hyprfetch doctor: OK (stub)");
            Ok(())
        }
    }
}
