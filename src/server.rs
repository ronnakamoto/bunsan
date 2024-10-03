use crate::config::{AppConfig, ChainNodeList};
use crate::error::Result;
use crate::load_balancer::LoadBalancingStrategy;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use arc_swap::ArcSwap;
use log::{error, info};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Chain {
    Ethereum,
    Optimism,
    Arbitrum,
    // Add more chains as needed
}

impl Chain {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "eth" | "ethereum" => Some(Chain::Ethereum),
            "op" | "optimism" => Some(Chain::Optimism),
            "arb" | "arbitrum" => Some(Chain::Arbitrum),
            // Add more mappings as needed
            _ => None,
        }
    }

    fn to_chain_id(&self) -> u64 {
        match self {
            Chain::Ethereum => 1,
            Chain::Optimism => 10,
            Chain::Arbitrum => 42161,
            // Add more mappings as needed
        }
    }
}

pub struct AppState {
    chain_nodes: Arc<ArcSwap<ChainNodeList>>,
    client: Client,
    load_balancers: Arc<HashMap<u64, Box<dyn LoadBalancingStrategy>>>,
}

async fn health_check(data: web::Data<AppState>) -> impl Responder {
    let chain_nodes = data.chain_nodes.load();
    let mut health_info = Vec::new();

    for (chain_id, nodes) in chain_nodes.iter() {
        let nodes = nodes.load();
        let healthy_count = nodes.nodes.iter().filter(|n| n.healthy).count();
        let total_count = nodes.nodes.len();
        let total_connections: usize = nodes.nodes.iter().map(|n| n.get_connections()).sum();
        health_info.push(json!({
            "chain_id": chain_id,
            "healthy_nodes": healthy_count,
            "total_nodes": total_count,
            "total_connections": total_connections
        }));
    }

    HttpResponse::Ok().json(json!({
        "status": "ok",
        "chains": health_info
    }))
}

async fn rpc_endpoint(
    req: HttpRequest,
    body: web::Json<Value>,
    data: web::Data<AppState>,
) -> HttpResponse {
    let chain = determine_chain(&req, &body);

    match chain {
        Some(chain) => process_request(chain, body, data).await,
        None => HttpResponse::BadRequest()
            .json(json!({"error": "Invalid or missing chain specification"})),
    }
}

fn determine_chain(req: &HttpRequest, body: &web::Json<Value>) -> Option<Chain> {
    // Check URL path
    if let Some(path) = req.match_info().get("chain") {
        if let Some(chain) = Chain::from_str(path) {
            return Some(chain);
        }
    }

    // Check header
    if let Some(chain_str) = req.headers().get("X-Chain-ID") {
        if let Ok(chain_str) = chain_str.to_str() {
            if let Some(chain) = Chain::from_str(chain_str) {
                return Some(chain);
            }
        }
    }

    // Check request body
    if let Some(params) = body.get("params") {
        if let Some(chain_obj) = params.get(0) {
            if let Some(chain_str) = chain_obj.get("chain") {
                if let Some(chain_str) = chain_str.as_str() {
                    return Chain::from_str(chain_str);
                }
            }
        }
    }

    // Default to Ethereum if no chain is specified
    Some(Chain::Ethereum)
}

async fn process_request(
    chain: Chain,
    body: web::Json<Value>,
    data: web::Data<AppState>,
) -> HttpResponse {
    let chain_id = chain.to_chain_id();
    let chain_nodes = data.chain_nodes.load();

    if let Some(nodes) = chain_nodes.get(&chain_id) {
        let nodes = nodes.load();
        if let Some(load_balancer) = data.load_balancers.get(&chain_id) {
            match load_balancer.select_node(&nodes.nodes) {
                Some(node) => {
                    node.increment_connections();
                    let response = data.client.post(&node.url).json(&body).send().await;
                    node.decrement_connections();

                    match response {
                        Ok(res) => match res.json::<Value>().await {
                            Ok(json) => HttpResponse::Ok().json(json),
                            Err(e) => {
                                error!("Failed to parse response from {}: {}", node.url, e);
                                HttpResponse::InternalServerError()
                                    .json(json!({"error": "Invalid response from node"}))
                            }
                        },
                        Err(e) => {
                            error!("Failed to forward request to {}: {}", node.url, e);
                            HttpResponse::InternalServerError()
                                .json(json!({"error": "Failed to forward request"}))
                        }
                    }
                }
                None => HttpResponse::ServiceUnavailable()
                    .json(json!({"error": "No available nodes for the specified chain"})),
            }
        } else {
            HttpResponse::BadRequest().json(json!({"error": "Invalid chain ID"}))
        }
    } else {
        HttpResponse::BadRequest().json(json!({"error": "Unsupported chain ID"}))
    }
}

// Macro to generate chain-specific endpoints
macro_rules! chain_endpoints {
    ($($chain:ident),*) => {
        $(
            pub async fn $chain(req: HttpRequest, body: web::Json<Value>, data: web::Data<AppState>) -> HttpResponse {
                rpc_endpoint(req, body, data).await
            }
        )*
    };
}

// Generate chain-specific endpoints
chain_endpoints!(ethereum, optimism, arbitrum);

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

pub async fn run_server(
    config: AppConfig,
    chain_nodes: Arc<ArcSwap<ChainNodeList>>,
    client: Client,
) -> Result<()> {
    let mut load_balancers = HashMap::new();
    for chain in &config.chains {
        load_balancers.insert(
            chain.chain_id,
            chain.load_balancing_strategy.create_strategy(),
        );
    }
    let load_balancers = Arc::new(load_balancers);

    let server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(AppState {
                chain_nodes: Arc::clone(&chain_nodes),
                client: client.clone(),
                load_balancers: Arc::clone(&load_balancers),
            }))
            .route("/health", web::get().to(health_check))
            .route("/", web::post().to(rpc_endpoint))
            .route("/{chain}", web::post().to(rpc_endpoint))
            .service(web::resource("/eth").route(web::post().to(ethereum)))
            .service(web::resource("/op").route(web::post().to(optimism)))
            .service(web::resource("/arb").route(web::post().to(arbitrum)))
    })
    .bind(&config.server_addr)?
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
