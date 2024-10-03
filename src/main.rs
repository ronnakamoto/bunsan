mod config;
mod error;
mod health;
mod load_balancer;
mod node;
mod server;

use crate::config::AppConfig;
use crate::error::Result;
use crate::health::update_node_health;
use crate::node::NodeList;
use crate::server::run_server;

use arc_swap::ArcSwap;
use clap::Parser;
use directories::ProjectDirs;
use env_logger::{self, Env};
use log::{error, info, warn};
use reqwest::Client;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::Duration;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the configuration file
    #[arg(short, long, env = "BUNSAN_CONFIG")]
    config: Option<PathBuf>,

    /// Delete the configuration file and exit
    #[arg(long)]
    delete_config: bool,
}

const DEFAULT_CONFIG: &str = r#"
server_addr = "127.0.0.1:8080"
update_interval = 60
load_balancing_strategy = "LeastConnections"

nodes = [
    "https://1rpc.io/eth",
    "https://eth.drpc.org",
    "https://eth.llamarpc.com",
    "https://ethereum.blockpi.network/v1/rpc/public",
]
"#;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let args = Args::parse();
    let config_path = args.config.unwrap_or_else(|| {
        ProjectDirs::from("com", "bunsan", "loadbalancer")
            .map(|proj_dirs| proj_dirs.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("config.toml"))
    });

    if args.delete_config {
        if config_path.exists() {
            fs::remove_file(&config_path)?;
            info!("Configuration file deleted: {}", config_path.display());
        } else {
            warn!(
                "Configuration file does not exist: {}",
                config_path.display()
            );
        }
        return Ok(());
    }

    info!("Starting Bunsan RPC Loadbalancer");
    info!("Using config file: {}", config_path.display());

    if !config_path.exists() {
        warn!("Config file not found. Creating a default config.");
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&config_path, DEFAULT_CONFIG)?;
        info!("Default config created at: {}", config_path.display());
    }

    let app_config = AppConfig::load(&config_path)?;
    let nodes = Arc::new(ArcSwap::from_pointee(NodeList::new(
        app_config.nodes.clone(),
    )));
    let client = Client::new();

    // Start the file watcher in a background task
    {
        let nodes_clone = Arc::clone(&nodes);
        let client_clone = client.clone();
        let config_path_clone = config_path.clone();

        tokio::spawn(async move {
            if let Err(e) =
                config::watch_config_file(&config_path_clone, nodes_clone, client_clone).await
            {
                error!("File watcher error: {}", e);
            }
        });
    }

    // Start the node health updater
    {
        let nodes_clone = Arc::clone(&nodes);
        let client_clone = client.clone();
        let update_interval = Duration::from_secs(app_config.update_interval);

        tokio::spawn(async move {
            if let Err(e) = update_node_health(nodes_clone, client_clone, update_interval).await {
                error!("Update nodes task failed: {}", e);
            }
        });
    }

    run_server(app_config, nodes, client).await?;

    Ok(())
}
