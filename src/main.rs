use std::{
    convert::TryFrom,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use config::{Config, File};
use env_logger::{self, Env};
use log::{error, info, warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::{sleep, timeout};

#[derive(Debug, Deserialize)]
struct AppConfig {
    server_addr: String,
    update_interval: u64,
    nodes: Vec<String>,
}

impl TryFrom<Config> for AppConfig {
    type Error = config::ConfigError;

    fn try_from(config: Config) -> std::result::Result<Self, Self::Error> {
        // Use `try_deserialize()` to avoid infinite recursion
        let app_config: AppConfig = config.try_deserialize()?;

        if app_config.update_interval == 0 {
            return Err(config::ConfigError::Message(
                "update_interval must be greater than 0".into(),
            ));
        }
        if app_config.nodes.is_empty() {
            return Err(config::ConfigError::Message(
                "At least one node must be specified".into(),
            ));
        }

        Ok(app_config)
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct NodeHealth {
    url: String,
    healthy: bool,
    last_check: DateTime<Utc>,
}

struct NodeList {
    nodes: Vec<NodeHealth>,
    index: AtomicUsize,
}

impl NodeList {
    fn new(nodes: Vec<String>) -> Self {
        let nodes = nodes
            .into_iter()
            .map(|url| NodeHealth {
                url,
                healthy: true,
                last_check: Utc::now(),
            })
            .collect();
        Self {
            nodes,
            index: AtomicUsize::new(0),
        }
    }

    fn get_next_node(&self) -> Option<&str> {
        let healthy_nodes: Vec<_> = self.nodes.iter().filter(|n| n.healthy).collect();
        if healthy_nodes.is_empty() {
            return None;
        }

        let index = self
            .index
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |idx| {
                Some((idx + 1) % healthy_nodes.len())
            })
            .unwrap_or(0);
        Some(&healthy_nodes[index].url)
    }
}

struct AppState {
    nodes: Arc<ArcSwap<NodeList>>,
    client: Client,
}

async fn health_check(data: web::Data<AppState>) -> impl Responder {
    let nodes = data.nodes.load();
    let healthy_count = nodes.nodes.iter().filter(|n| n.healthy).count();
    let total_count = nodes.nodes.len();
    HttpResponse::Ok().json(json!({
        "status": "ok",
        "healthy_nodes": healthy_count,
        "total_nodes": total_count
    }))
}

async fn rpc_endpoint(body: web::Json<Value>, data: web::Data<AppState>) -> HttpResponse {
    let nodes = data.nodes.load();
    match nodes.get_next_node() {
        Some(evm_node) => {
            let response = data.client.post(evm_node).json(&body).send().await;

            match response {
                Ok(res) => match res.json::<Value>().await {
                    Ok(json) => HttpResponse::Ok().json(json),
                    Err(e) => {
                        error!("Failed to parse response from {}: {}", evm_node, e);
                        HttpResponse::InternalServerError()
                            .json(json!({"error": "Invalid response from node"}))
                    }
                },
                Err(e) => {
                    error!("Failed to forward request to {}: {}", evm_node, e);
                    HttpResponse::InternalServerError()
                        .json(json!({"error": "Failed to forward request"}))
                }
            }
        }
        None => HttpResponse::ServiceUnavailable().json(json!({"error": "No available nodes"})),
    }
}

async fn update_nodes(
    nodes: Arc<ArcSwap<NodeList>>,
    client: Client,
    interval: Duration,
) -> Result<()> {
    loop {
        sleep(interval).await;

        // Reload the configuration
        let config = Config::builder()
            .add_source(File::with_name("config.toml"))
            .build()
            .context("Failed to reload configuration")?;
        let app_config: AppConfig = config.try_deserialize()?;

        // Use the updated node list from the configuration
        let mut node_health = Vec::new();
        for url in app_config.nodes {
            let health = check_node_health(&client, &url).await;
            node_health.push(NodeHealth {
                url,
                healthy: health,
                last_check: Utc::now(),
            });
        }

        let new_node_list = Arc::new(NodeList {
            nodes: node_health,
            index: AtomicUsize::new(0),
        });

        nodes.store(new_node_list);

        info!(
            "Updated node list. New node count: {}",
            nodes.load().nodes.len()
        );
    }

    // The loop is infinite; this return is unreachable but required by the compiler
    #[allow(unreachable_code)]
    Ok(())
}

async fn check_node_health(client: &Client, url: &str) -> bool {
    const MAX_RETRIES: u32 = 3;
    const TIMEOUT_DURATION: Duration = Duration::from_secs(5);

    for attempt in 0..MAX_RETRIES {
        let health_check_payload = json!({
            "jsonrpc": "2.0",
            "method": "web3_clientVersion",
            "params": [],
            "id": 1
        });

        let result = timeout(
            TIMEOUT_DURATION,
            client.post(url).json(&health_check_payload).send(),
        )
        .await;

        match result {
            Ok(Ok(response)) if response.status().is_success() => return true,
            Ok(Ok(response)) => {
                warn!(
                    "Node {} responded with non-success status: {}",
                    url,
                    response.status()
                );
            }
            Ok(Err(e)) => {
                warn!("Request error when checking node {}: {}", url, e);
            }
            Err(_) => {
                warn!("Timeout when checking node {}", url);
            }
        }

        if attempt < MAX_RETRIES - 1 {
            tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
        } else {
            warn!(
                "Health check failed for node {} after {} attempts",
                url, MAX_RETRIES
            );
            return false;
        }
    }
    false
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received, starting graceful shutdown");
}

#[actix_web::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    info!("Starting Bunsan RPC Loadbalancer");

    let config = Config::builder()
        .add_source(File::with_name("config.toml"))
        .build()
        .context("Failed to load configuration")?;
    let app_config: AppConfig = AppConfig::try_from(config).context("Invalid configuration")?;

    let nodes = Arc::new(ArcSwap::from_pointee(NodeList::new(
        app_config.nodes.clone(),
    )));
    let nodes_clone = Arc::clone(&nodes);

    let client = Client::new();
    let client_clone = client.clone();

    tokio::spawn(async move {
        if let Err(e) = update_nodes(
            nodes_clone,
            client_clone,
            Duration::from_secs(app_config.update_interval),
        )
        .await
        {
            error!("Update nodes task failed: {}", e);
        }
    });

    let server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(AppState {
                nodes: Arc::clone(&nodes),
                client: client.clone(),
            }))
            .route("/health", web::get().to(health_check))
            .route("/", web::post().to(rpc_endpoint))
    })
    .bind(&app_config.server_addr)
    .context("Failed to bind to address")?
    .run();

    tokio::select! {
        result = server => {
            if let Err(e) = result {
                error!("Server error: {}", e);
            }
        },
        _ = shutdown_signal() => {
            info!("Shutting down server");
        },
    }

    Ok(())
}
