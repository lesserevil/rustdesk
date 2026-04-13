mod protocol;
mod ws_server;

use clap::Parser;
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Parser)]
#[command(
    name = "ctap-companion",
    about = "RustDesk CTAP Companion — bridges FIDO2 security keys to the RustDesk web client"
)]
struct Args {
    /// WebSocket listen port
    #[arg(long, default_value_t = 21118)]
    port: u16,

    /// Additional allowed origin (repeatable)
    #[arg(long = "allowed-origin")]
    allowed_origins: Vec<String>,

    /// Enable debug logging
    #[arg(long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    env_logger::Builder::new()
        .filter_level(if args.verbose {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        })
        .init();

    let shutdown = Arc::new(Notify::new());

    // Handle Ctrl+C
    let shutdown_signal = shutdown.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        log::info!("Received Ctrl+C, shutting down");
        shutdown_signal.notify_one();
    });

    let server = ws_server::WsServer::new(args.port, args.allowed_origins);

    // Try primary port, then fallback
    match server.run(shutdown.clone()).await {
        Ok(()) => {}
        Err(e) => {
            if args.port == 21118 {
                log::warn!("Port 21118 failed ({}), trying 21119", e);
                let server = ws_server::WsServer::new(21119, vec![]);
                server.run(shutdown).await?;
            } else {
                return Err(e);
            }
        }
    }

    Ok(())
}
