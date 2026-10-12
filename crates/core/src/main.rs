use clap::{Parser, Subcommand};
use serialbridge_core::{
    config::{self, Config},
    transport::TransportMode,
};
use std::{net::SocketAddr, path::PathBuf};
#[derive(Parser)]
#[command(
    version,
    about = "SerialBridge peer node — loopback HTTP over USB serial"
)]
struct Args {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Initialize a config file. The token is not printed unless explicitly requested.
    Init,
    /// Run the headless node. Do not run alongside the desktop using the same port.
    Serve {
        #[arg(long, conflicts_with = "dev_tcp_listen")]
        dev_tcp_connect: Option<SocketAddr>,
        #[arg(long)]
        dev_tcp_listen: Option<SocketAddr>,
    },
    Ports,
    /// Display the local API credential. Never share it with a remote peer.
    Token,
    Status,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let path = args.config.unwrap_or_else(config::default_path);
    tracing_subscriber::fmt()
        .with_env_filter("serialbridge_core=info")
        .with_writer(std::io::stderr)
        .init();
    match args.command {
        Command::Init => {
            Config::load(&path)?;
            println!("Configuration initialized: {}", path.display());
        }
        Command::Token => {
            let cfg = Config::load(&path)?;
            println!("{}", cfg.api_token);
        }
        Command::Ports => {
            for p in tokio_serial::available_ports()? {
                println!("{}  {:?}", p.port_name, p.port_type);
            }
        }
        Command::Status => {
            let cfg = Config::load(&path)?;
            let value: serde_json::Value = reqwest::Client::new()
                .get(format!("http://127.0.0.1:{}/v1/health", cfg.http_port))
                .bearer_auth(cfg.api_token)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        Command::Serve {
            dev_tcp_connect,
            dev_tcp_listen,
        } => {
            let mode = match (dev_tcp_connect, dev_tcp_listen) {
                (Some(a), _) => TransportMode::TcpConnect(a),
                (_, Some(a)) => TransportMode::TcpListen(a),
                _ => TransportMode::Serial,
            };
            let node = serialbridge_core::start(path, mode).await?;
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=node.shutdown.cancelled()=>{}}
            node.shutdown.cancel();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }
    Ok(())
}
