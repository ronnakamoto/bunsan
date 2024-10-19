use crate::config::{AppConfig, ChainNodeList};
use crate::error::Result;
use crate::extensions::manager::{ExtensionManager, RouteConfig};
use crate::load_balancer::LoadBalancingStrategy;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder, Scope};
use arc_swap::ArcSwap;
use log::{error, info, warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Chain {
    Ethereum,
    Optimism,
    Arbitrum,
    BNBSmartChain,
    BNBSmartChainTestnet,
    SepoliaTestnet,
    OPSepoliaTestnet,
    ArbitrumSepoliaTestnet,
    BaseSepoliaTestnet,
    Base,
    PolygonMainnet,
    PolygonZkEVM,
    PolygonAmoy,
    PolygonZkEVMTestnet,
    Scroll,
    ScrollSepoliaTestnet,
    TaikoMainnet,
    NeonEVMMainnet,
    NeonEVMDevnet,
}

impl Chain {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "eth" | "ethereum" => Some(Chain::Ethereum),
            "op" | "optimism" => Some(Chain::Optimism),
            "arb" | "arbitrum" => Some(Chain::Arbitrum),
            "bnb" | "binance" | "bsc" => Some(Chain::BNBSmartChain),
            "bnbt" | "bsc-testnet" => Some(Chain::BNBSmartChainTestnet),
            "sepolia" => Some(Chain::SepoliaTestnet),
            "op-sepolia" => Some(Chain::OPSepoliaTestnet),
            "arb-sepolia" => Some(Chain::ArbitrumSepoliaTestnet),
            "base-sepolia" => Some(Chain::BaseSepoliaTestnet),
            "base" => Some(Chain::Base),
            "polygon" | "matic" => Some(Chain::PolygonMainnet),
            "polygon-zkevm" => Some(Chain::PolygonZkEVM),
            "polygon-amoy" => Some(Chain::PolygonAmoy),
            "polygon-zkevm-testnet" => Some(Chain::PolygonZkEVMTestnet),
            "scroll" => Some(Chain::Scroll),
            "scroll-sepolia" => Some(Chain::ScrollSepoliaTestnet),
            "taiko" => Some(Chain::TaikoMainnet),
            "neon" => Some(Chain::NeonEVMMainnet),
            "neon-devnet" => Some(Chain::NeonEVMDevnet),
            _ => None,
        }
    }

    fn to_chain_id(&self) -> u64 {
        match self {
            Chain::Ethereum => 1,
            Chain::Optimism => 10,
            Chain::Arbitrum => 42161,
            Chain::BNBSmartChain => 56,
            Chain::BNBSmartChainTestnet => 97,
            Chain::SepoliaTestnet => 11155111,
            Chain::OPSepoliaTestnet => 11155420,
            Chain::ArbitrumSepoliaTestnet => 421614,
            Chain::BaseSepoliaTestnet => 84532,
            Chain::Base => 8453,
            Chain::PolygonMainnet => 137,
            Chain::PolygonZkEVM => 1101,
            Chain::PolygonAmoy => 80002,
            Chain::PolygonZkEVMTestnet => 1442,
            Chain::Scroll => 534352,
            Chain::ScrollSepoliaTestnet => 534351,
            Chain::TaikoMainnet => 167000,
            Chain::NeonEVMMainnet => 245022934,
            Chain::NeonEVMDevnet => 245022926,
        }
    }
}

#[derive(Deserialize)]
struct TransactionPath {
    chain: Option<String>,
    tx_hash: String,
}

pub struct ExtensionState {
    pub manager: Arc<ExtensionManager>,
    pub routes: HashMap<String, Vec<RouteConfig>>,
}

pub struct AppState {
    chain_nodes: Arc<ArcSwap<ChainNodeList>>,
    client: Client,
    load_balancers: Arc<HashMap<u64, Box<dyn LoadBalancingStrategy>>>,
    extension_state: Arc<ExtensionState>,
}

fn match_path(route_path: &str, actual_path: &str) -> Option<HashMap<String, String>> {
    let route_segments: Vec<&str> = route_path.split('/').collect();
    let actual_segments: Vec<&str> = actual_path.split('/').collect();

    if route_segments.len() != actual_segments.len() {
        return None;
    }

    let mut params = HashMap::new();

    for (route_seg, actual_seg) in route_segments.iter().zip(actual_segments.iter()) {
        if route_seg.starts_with('{') && route_seg.ends_with('}') {
            let param_name = &route_seg[1..route_seg.len() - 1];
            params.insert(param_name.to_string(), actual_seg.to_string());
        } else if *route_seg != *actual_seg {
            return None;
        }
    }

    Some(params)
}

fn extract_path_params(route_path: &str, actual_path: &str) -> HashMap<String, String> {
    match_path(route_path, actual_path).unwrap_or_default()
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

async fn get_transaction_details(
    req: HttpRequest,
    path: web::Path<TransactionPath>,
    query: web::Query<HashMap<String, String>>,
    data: web::Data<AppState>,
) -> HttpResponse {
    let TransactionPath {
        chain: chain_path,
        tx_hash,
    } = path.into_inner();

    // Determine the chain from the path, header, or default
    let chain = if let Some(chain_str) = chain_path {
        Chain::from_str(&chain_str)
    } else {
        let empty_body = web::Json(json!({}));
        determine_chain(&req, &empty_body)
    };

    // Parse the fields to filter
    let fields: HashSet<String> = query
        .get("fields")
        .map(|f| f.split(',').map(String::from).collect())
        .unwrap_or_else(HashSet::new);

    match chain {
        Some(chain) => {
            let chain_id = chain.to_chain_id();
            let chain_nodes = data.chain_nodes.load();

            if let Some(nodes) = chain_nodes.get(&chain_id) {
                let nodes = nodes.load();
                if let Some(load_balancer) = data.load_balancers.get(&chain_id) {
                    match load_balancer.select_node(&nodes.nodes) {
                        Some(node) => {
                            node.increment_connections();
                            let result = send_request(
                                &data.client,
                                &node.url,
                                &json!({
                                    "jsonrpc": "2.0",
                                    "method": "eth_getTransactionByHash",
                                    "params": [tx_hash],
                                    "id": 1
                                }),
                                3,
                            )
                            .await;
                            node.decrement_connections();

                            match result {
                                Ok(json) => {
                                    let filtered_json = filter_json_fields(json, &fields);
                                    HttpResponse::Ok().json(filtered_json)
                                }
                                Err(e) => {
                                    error!("Failed to fetch transaction details: {}", e);
                                    HttpResponse::InternalServerError().json(
                                        json!({"error": "Failed to fetch transaction details"}),
                                    )
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
        None => HttpResponse::BadRequest()
            .json(json!({"error": "Invalid or missing chain specification"})),
    }
}

fn filter_json_fields(json: Value, fields: &HashSet<String>) -> Value {
    if fields.is_empty() {
        return json;
    }

    match json {
        Value::Object(map) => {
            let filtered_map: serde_json::Map<String, Value> = map
                .into_iter()
                .filter_map(|(k, v)| {
                    if k == "result" {
                        Some((k, filter_json_fields(v, fields)))
                    } else if fields.contains(&k) {
                        Some((k, v))
                    } else {
                        None
                    }
                })
                .collect();
            Value::Object(filtered_map)
        }
        Value::Array(arr) => Value::Array(
            arr.into_iter()
                .map(|v| filter_json_fields(v, fields))
                .collect(),
        ),
        _ => json,
    }
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
                    let result = send_request(&data.client, &node.url, &body, 3).await;
                    node.decrement_connections();

                    match result {
                        Ok(json) => HttpResponse::Ok().json(json),
                        Err(e) => {
                            error!("Failed to process request for {}: {}", node.url, e);
                            HttpResponse::InternalServerError()
                                .json(json!({"error": "Failed to process request"}))
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

async fn send_request(
    client: &Client,
    url: &str,
    body: &Value,
    max_retries: usize,
) -> Result<Value> {
    let mut retries = 0;
    let mut last_error = None;

    while retries < max_retries {
        match timeout(Duration::from_secs(10), client.post(url).json(body).send()).await {
            Ok(Ok(response)) => {
                if response.status().is_success() {
                    return Ok(response.json().await?);
                } else {
                    warn!("Received non-success status code: {}", response.status());
                }
            }
            Ok(Err(e)) => {
                warn!("Request failed: {}", e);
                last_error = Some(e.into());
            }
            Err(_) => {
                warn!("Request timed out");
                last_error = Some(anyhow::anyhow!("Request timed out").into());
            }
        }

        retries += 1;
        if retries < max_retries {
            tokio::time::sleep(Duration::from_millis(100 * 2u64.pow(retries as u32))).await;
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Max retries reached").into()))
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
chain_endpoints!(
    ethereum,
    optimism,
    arbitrum,
    bnb_smart_chain,
    bnb_smart_chain_testnet,
    sepolia_testnet,
    op_sepolia_testnet,
    arbitrum_sepolia_testnet,
    base_sepolia_testnet,
    base,
    polygon_mainnet,
    polygon_zkevm,
    polygon_amoy,
    polygon_zkevm_testnet,
    scroll,
    scroll_sepolia_testnet,
    taiko_mainnet,
    neon_evm_mainnet,
    neon_evm_devnet
);

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

async fn handle_extension_request(
    data: web::Data<AppState>,
    req: HttpRequest,
    path: web::Path<(String, String)>,
    query: web::Query<HashMap<String, String>>,
    body: Option<web::Json<Value>>,
) -> HttpResponse {
    let (extension_name, route_path) = path.into_inner();
    let routes = &data.extension_state.routes;

    if let Some(extension_routes) = routes.get(&extension_name) {
        if let Some(route) = extension_routes.iter().find(|r| {
            let path_match = match_path(&r.path, &route_path);
            path_match.is_some() && r.method == req.method().as_str()
        }) {
            let headers: HashMap<String, String> = req
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_string(),
                        value.to_str().unwrap_or("").to_string(),
                    )
                })
                .collect();

            let body_value = body.map(|b| b.into_inner()).unwrap_or(Value::Null);

            let path_params = extract_path_params(&route.path, &route_path);

            match data
                .extension_state
                .manager
                .run_extension(
                    &extension_name,
                    route,
                    &headers,
                    &body_value,
                    &query.into_inner(),
                    &path_params,
                )
                .await
            {
                Ok(output) => HttpResponse::Ok().body(output),
                Err(e) => {
                    error!("Extension execution failed: {}", e);
                    HttpResponse::InternalServerError()
                        .body(format!("Extension execution failed: {}", e))
                }
            }
        } else {
            HttpResponse::NotFound().body("Route not found")
        }
    } else {
        HttpResponse::NotFound().body("Extension not found")
    }
}

pub async fn run_server(
    config: AppConfig,
    chain_nodes: Arc<ArcSwap<ChainNodeList>>,
    client: Client,
    extension_state: Arc<ExtensionState>,
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
        let app = App::new()
            .app_data(web::Data::new(AppState {
                chain_nodes: Arc::clone(&chain_nodes),
                client: client.clone(),
                load_balancers: Arc::clone(&load_balancers),
                extension_state: Arc::clone(&extension_state),
            }))
            .route("/health", web::get().to(health_check))
            .route("/", web::post().to(rpc_endpoint))
            .route("/{chain}", web::post().to(rpc_endpoint))
            .route("/tx/{tx_hash}", web::get().to(get_transaction_details))
            .route(
                "/{chain}/tx/{tx_hash}",
                web::get().to(get_transaction_details),
            )
            .service(web::resource("/eth").route(web::post().to(ethereum)))
            .service(web::resource("/op").route(web::post().to(optimism)))
            .service(web::resource("/arb").route(web::post().to(arbitrum)))
            .service(web::resource("/bnb").route(web::post().to(bnb_smart_chain)))
            .service(web::resource("/bnbt").route(web::post().to(bnb_smart_chain_testnet)))
            .service(web::resource("/sepolia").route(web::post().to(sepolia_testnet)))
            .service(web::resource("/op-sepolia").route(web::post().to(op_sepolia_testnet)))
            .service(web::resource("/arb-sepolia").route(web::post().to(arbitrum_sepolia_testnet)))
            .service(web::resource("/base-sepolia").route(web::post().to(base_sepolia_testnet)))
            .service(web::resource("/base").route(web::post().to(base)))
            .service(web::resource("/polygon").route(web::post().to(polygon_mainnet)))
            .service(web::resource("/polygon-zkevm").route(web::post().to(polygon_zkevm)))
            .service(web::resource("/polygon-amoy").route(web::post().to(polygon_amoy)))
            .service(
                web::resource("/polygon-zkevm-testnet")
                    .route(web::post().to(polygon_zkevm_testnet)),
            )
            .service(web::resource("/scroll").route(web::post().to(scroll)))
            .service(web::resource("/scroll-sepolia").route(web::post().to(scroll_sepolia_testnet)))
            .service(web::resource("/taiko").route(web::post().to(taiko_mainnet)))
            .service(web::resource("/neon").route(web::post().to(neon_evm_mainnet)))
            .service(web::resource("/neon-devnet").route(web::post().to(neon_evm_devnet)));

        // Add extension routes
        let extensions_scope = extension_state.routes.iter().fold(
            Scope::new("/extensions"),
            |extensions_scope, (extension_name, routes)| {
                let extension_scope =
                    routes
                        .iter()
                        .fold(Scope::new(extension_name), |extension_scope, route| {
                            extension_scope.route(
                                &route.path,
                                match route.method.as_str() {
                                    "GET" => web::get().to(handle_extension_request),
                                    "POST" => web::post().to(handle_extension_request),
                                    "PUT" => web::put().to(handle_extension_request),
                                    "DELETE" => web::delete().to(handle_extension_request),
                                    _ => web::get().to(|| HttpResponse::MethodNotAllowed()),
                                },
                            )
                        });
                extensions_scope.service(extension_scope)
            },
        );

        app.service(extensions_scope)
    })
    .bind(&config.server_addr)?
    .run();

    info!("Server running at http://{}/", config.server_addr);

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
