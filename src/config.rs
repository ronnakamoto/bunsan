use crate::error::Result;
use crate::load_balancer::StrategyType;
use crate::node::{NodeHealth, NodeList};
use crate::server::Chain;
use arc_swap::ArcSwap;
use config::{Config, File};
use log::{error, info};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Deserialize, Serialize)]
pub struct ChainConfig {
    pub name: String,
    pub chain_id: u64,
    pub chain: Option<Chain>,
    pub nodes: Vec<String>,
    pub load_balancing_strategy: StrategyType,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ConnectionPoolConfig {
    pub max_size: u32,
    pub min_idle: Option<u32>,
    pub max_lifetime: Option<u64>,
    pub idle_timeout: Option<u64>,
    pub connection_timeout: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AppConfig {
    pub server_addr: String,
    pub update_interval: u64,
    pub chains: Vec<ChainConfig>,
    pub connection_pool: ConnectionPoolConfig,
}

impl AppConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let config = Config::builder()
            .add_source(File::from(path.as_ref()))
            .build()?;

        let mut app_config: AppConfig = config.try_deserialize()?;

        // Assign the Chain enum based on the chain field or chain_id
        for chain_config in &mut app_config.chains {
            if chain_config.chain.is_none() {
                chain_config.chain = match chain_config.chain_id {
                    1 => Some(Chain::Ethereum),
                    1313161555 => Some(Chain::AuroraTestnet),
                    10 => Some(Chain::Optimism),
                    42161 => Some(Chain::Arbitrum),
                    56 => Some(Chain::BNBSmartChain),
                    97 => Some(Chain::BNBSmartChainTestnet),
                    11155111 => Some(Chain::SepoliaTestnet),
                    11155420 => Some(Chain::OPSepoliaTestnet),
                    421614 => Some(Chain::ArbitrumSepoliaTestnet),
                    84532 => Some(Chain::BaseSepoliaTestnet),
                    8453 => Some(Chain::Base),
                    137 => Some(Chain::PolygonMainnet),
                    1101 => Some(Chain::PolygonZkEVM),
                    80002 => Some(Chain::PolygonAmoy),
                    1442 => Some(Chain::PolygonZkEVMTestnet),
                    534352 => Some(Chain::Scroll),
                    534351 => Some(Chain::ScrollSepoliaTestnet),
                    167000 => Some(Chain::TaikoMainnet),
                    245022934 => Some(Chain::NeonEVMMainnet),
                    245022926 => Some(Chain::NeonEVMDevnet),
                    _ => {
                        return Err(anyhow::anyhow!(
                            "Unsupported chain ID: {}",
                            chain_config.chain_id
                        ))
                    }
                };
            }
        }

        // Validate configuration
        Self::validate(&app_config)?;

        Ok(app_config)
    }

    fn validate(config: &AppConfig) -> Result<()> {
        if config.update_interval == 0 {
            return Err(anyhow::anyhow!("update_interval must be greater than 0"));
        }
        if config.chains.is_empty() {
            return Err(anyhow::anyhow!("At least one chain must be specified"));
        }
        for chain in &config.chains {
            if chain.nodes.is_empty() {
                return Err(anyhow::anyhow!(
                    "At least one node must be specified for each chain"
                ));
            }
        }
        if config.connection_pool.max_size == 0 {
            return Err(anyhow::anyhow!(
                "connection_pool.max_size must be greater than 0"
            ));
        }
        Ok(())
    }
}

pub type ChainNodeList = HashMap<u64, Arc<ArcSwap<NodeList>>>;

pub async fn watch_config_file<P: AsRef<Path>>(
    path: P,
    chain_nodes: Arc<ArcSwap<ChainNodeList>>,
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

            if let Err(e) = reload_configuration(&path, &chain_nodes, &client).await {
                error!("Failed to reload configuration: {}", e);
            }
        }
    }

    Ok(())
}

async fn reload_configuration<P: AsRef<Path>>(
    path: P,
    chain_nodes: &Arc<ArcSwap<ChainNodeList>>,
    client: &Client,
) -> Result<()> {
    let app_config = AppConfig::load(path)?;

    let mut new_chain_nodes = ChainNodeList::new();

    for chain in &app_config.chains {
        let mut node_health = Vec::new();
        for url in &chain.nodes {
            let health = crate::health::check_node_health(client, url).await;
            node_health.push(NodeHealth::new(url.clone(), health));
        }

        let new_node_list = Arc::new(ArcSwap::from_pointee(NodeList::new_with_health(
            node_health,
        )));
        new_chain_nodes.insert(chain.chain_id, new_node_list);
    }

    chain_nodes.store(Arc::new(new_chain_nodes));

    info!(
        "Configuration reloaded. New chain count: {}",
        app_config.chains.len()
    );

    Ok(())
}

pub fn create_client(config: &ConnectionPoolConfig) -> Client {
    Client::builder()
        .timeout(Duration::from_millis(config.connection_timeout))
        .pool_max_idle_per_host(config.min_idle.map(|v| v as usize).unwrap_or(0))
        .pool_idle_timeout(config.idle_timeout.map(Duration::from_secs))
        .pool_max_idle_per_host(config.max_size as usize)
        .build()
        .expect("Failed to create HTTP client")
}
