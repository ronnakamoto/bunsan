use crate::error::Result;
use crate::load_balancer::StrategyType;
use crate::node::{NodeHealth, NodeList};
use arc_swap::ArcSwap;
use config::{Config, File};
use log::{error, info};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use reqwest::Client;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub server_addr: String,
    pub update_interval: u64,
    pub nodes: Vec<String>,
    pub load_balancing_strategy: StrategyType,
}

impl AppConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let config = Config::builder()
            .add_source(File::from(path.as_ref()))
            .build()?;

        let app_config: AppConfig = config.try_deserialize()?;

        if app_config.update_interval == 0 {
            return Err(anyhow::anyhow!("update_interval must be greater than 0"));
        }
        if app_config.nodes.is_empty() {
            return Err(anyhow::anyhow!("At least one node must be specified"));
        }

        Ok(app_config)
    }
}

pub async fn watch_config_file<P: AsRef<Path>>(
    path: P,
    nodes: Arc<ArcSwap<NodeList>>,
    client: Client,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel(1);

    let mut watcher = RecommendedWatcher::new(
        move |res| {
            if let Ok(event) = res {
                let _ = tx.try_send(event);
            }
        },
        notify::Config::default(),
    )?;

    watcher.watch(path.as_ref(), RecursiveMode::NonRecursive)?;

    while let Some(event) = rx.recv().await {
        if matches!(event.kind, EventKind::Modify(_)) {
            info!("Configuration file changed, reloading...");

            if let Err(e) = reload_configuration(&path, &nodes, &client).await {
                error!("Failed to reload configuration: {}", e);
            }
        }
    }

    Ok(())
}

async fn reload_configuration<P: AsRef<Path>>(
    path: P,
    nodes: &Arc<ArcSwap<NodeList>>,
    client: &Client,
) -> Result<()> {
    let app_config = AppConfig::load(path)?;

    let mut node_health = Vec::new();
    for url in app_config.nodes {
        let health = crate::health::check_node_health(client, &url).await;
        node_health.push(NodeHealth::new(url, health));
    }

    let new_node_list = Arc::new(NodeList::new_with_health(node_health));

    nodes.store(new_node_list);

    info!(
        "Configuration reloaded. New node count: {}",
        nodes.load().nodes.len()
    );

    Ok(())
}
