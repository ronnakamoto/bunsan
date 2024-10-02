mod config;
mod error;
mod health;
mod node;
mod server;

use crate::config::AppConfig;
use crate::error::Result;
use crate::health::update_node_health;
use crate::node::NodeList;
use crate::server::run_server;

use arc_swap::ArcSwap;
use env_logger::{self, Env};
use log::{error, info};
use reqwest::Client;
use std::sync::Arc;
use tokio::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    info!("Starting Bunsan RPC Loadbalancer");

    let app_config = AppConfig::load("config.toml")?;
    let nodes = Arc::new(ArcSwap::from_pointee(NodeList::new(
        app_config.nodes.clone(),
    )));
    let client = Client::new();

    // Start the file watcher in a background task
    {
        let nodes_clone = Arc::clone(&nodes);
        let client_clone = client.clone();

        tokio::spawn(async move {
            if let Err(e) =
                config::watch_config_file("config.toml", nodes_clone, client_clone).await
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
