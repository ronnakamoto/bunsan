use crate::config::{AppConfig, ChainNodeList};
use crate::error::Result;
use crate::extensions::manager::{ArrayStyle, ExtensionManager, ParameterSource, RouteConfig};
use crate::load_balancer::LoadBalancingStrategy;
use actix_web::{
    web, App, HttpRequest, HttpResponse, HttpServer, Responder, Result as ActixResult,
};
use arc_swap::ArcSwap;
use chrono::{DateTime, TimeZone, Utc};
use log::{debug, error, info, warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::time::timeout;
use toml_edit::DocumentMut;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Chain {
    Ethereum,
    AuroraTestnet,
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
            "aurora-testnet" | "aurora-test" => Some(Chain::AuroraTestnet),
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
            Chain::AuroraTestnet => 1313161555,
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

fn extract_path_params(route_path: &str, actual_path: &str) -> HashMap<String, String> {
    let route_segments: Vec<&str> = route_path.split('/').collect();
    let actual_segments: Vec<&str> = actual_path.split('/').collect();
    let mut params = HashMap::new();

    for (route_seg, actual_seg) in route_segments.iter().zip(actual_segments.iter()) {
        if route_seg.starts_with('{') && route_seg.ends_with('}') {
            let param_name = &route_seg[1..route_seg.len() - 1];
            params.insert(param_name.to_string(), actual_seg.to_string());
        }
    }

    params
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
    debug!(
        "Received RPC request:\nPath: {}\nMethod: {}\nHeaders: {:?}\nBody: {}",
        req.uri().path(),
        req.method(),
        req.headers(),
        serde_json::to_string_pretty(&body).unwrap_or_default()
    );
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

#[derive(Debug, Serialize, Deserialize)]
pub struct EventLog {
    pub address: String,
    pub topics: Vec<String>,
    pub data: String,
    pub block_number: String,
    pub transaction_hash: String,
    pub transaction_index: String,
    pub block_hash: String,
    pub log_index: String,
    pub removed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decoded_event: Option<DecodedEvent>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DecodedEvent {
    pub name: String,
    pub params: Vec<DecodedParam>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DecodedParam {
    pub name: String,
    pub value: String,
    pub indexed: bool,
}

async fn get_transaction_events(
    req: HttpRequest,
    path: web::Path<TransactionPath>,
    _query: web::Query<HashMap<String, String>>,
    data: web::Data<AppState>,
) -> HttpResponse {
    let TransactionPath {
        chain: chain_path,
        tx_hash,
    } = path.into_inner();

    // Determine the chain (reusing existing logic)
    let chain = if let Some(chain_str) = chain_path {
        Chain::from_str(&chain_str)
    } else {
        let empty_body = web::Json(json!({}));
        determine_chain(&req, &empty_body)
    };

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

                            // First, get the transaction receipt
                            debug!("Fetching transaction receipt for hash: {}", tx_hash);
                            let receipt_result = send_request(
                                &data.client,
                                &node.url,
                                &json!({
                                    "jsonrpc": "2.0",
                                    "method": "eth_getTransactionReceipt",
                                    "params": [tx_hash],
                                    "id": 1
                                }),
                                3,
                            )
                            .await;

                            // Get block timestamp if receipt exists
                            let mut block_timestamp = None;
                            if let Ok(ref receipt) = receipt_result {
                                debug!("Received transaction receipt: {:?}", receipt);
                                if let Some(block_number) =
                                    receipt["result"]["blockNumber"].as_str()
                                {
                                    debug!(
                                        "Fetching block details for block number: {}",
                                        block_number
                                    );
                                    let block_result = send_request(
                                        &data.client,
                                        &node.url,
                                        &json!({
                                            "jsonrpc": "2.0",
                                            "method": "eth_getBlockByNumber",
                                            "params": [block_number, false],
                                            "id": 1
                                        }),
                                        3,
                                    )
                                    .await;

                                    if let Ok(block) = block_result {
                                        debug!("Received block details: {:?}", block);
                                        if let Some(timestamp_hex) =
                                            block["result"]["timestamp"].as_str()
                                        {
                                            if let Ok(timestamp) = u64::from_str_radix(
                                                timestamp_hex.trim_start_matches("0x"),
                                                16,
                                            ) {
                                                match Utc.timestamp_opt(timestamp as i64, 0) {
                                                    chrono::LocalResult::Single(dt) => {
                                                        block_timestamp = Some(dt);
                                                        debug!("Parsed block timestamp: {:?}", dt);
                                                    }
                                                    chrono::LocalResult::None => {
                                                        warn!(
                                                            "Invalid timestamp value: {}",
                                                            timestamp
                                                        );
                                                        // Fallback to Unix epoch
                                                        if let chrono::LocalResult::Single(dt) =
                                                            Utc.timestamp_opt(0, 0)
                                                        {
                                                            block_timestamp = Some(dt);
                                                            warn!("Using Unix epoch as fallback timestamp");
                                                        }
                                                    }
                                                    chrono::LocalResult::Ambiguous(dt1, _) => {
                                                        warn!("Ambiguous timestamp value: {}. Using first option.", timestamp);
                                                        block_timestamp = Some(dt1);
                                                    }
                                                }
                                            } else {
                                                warn!(
                                                    "Failed to parse timestamp hex: {}",
                                                    timestamp_hex
                                                );
                                            }
                                        } else {
                                            warn!("No timestamp found in block data");
                                        }
                                    } else if let Err(e) = block_result {
                                        warn!("Failed to fetch block details: {}", e);
                                    }
                                }
                            }

                            node.decrement_connections();

                            match receipt_result {
                                Ok(receipt) => {
                                    if let Some(result) = receipt.get("result") {
                                        if let Some(logs) =
                                            result.get("logs").and_then(|l| l.as_array())
                                        {
                                            debug!("Processing {} log entries", logs.len());
                                            let events: Vec<EventLog> = logs
                                                .iter()
                                                .filter_map(|log| {
                                                    debug!("Processing log entry: {:?}", log);
                                                    match serde_json::from_value::<EventLog>(
                                                        log.clone(),
                                                    ) {
                                                        Ok(mut event_log) => {
                                                            event_log.timestamp = block_timestamp;
                                                            debug!("Successfully parsed log entry");
                                                            Some(event_log)
                                                        }
                                                        Err(e) => {
                                                            error!(
                                                                "Failed to parse log entry: {}",
                                                                e
                                                            );
                                                            error!("Raw log entry: {:?}", log);
                                                            // Attempt to construct event log manually
                                                            let event_log = EventLog {
                                                                address: log["address"]
                                                                    .as_str()
                                                                    .unwrap_or("")
                                                                    .to_string(),
                                                                topics: log["topics"]
                                                                    .as_array()
                                                                    .map(|t| {
                                                                        t.iter()
                                                                            .filter_map(|topic| {
                                                                                topic.as_str().map(
                                                                                    String::from,
                                                                                )
                                                                            })
                                                                            .collect()
                                                                    })
                                                                    .unwrap_or_default(),
                                                                data: log["data"]
                                                                    .as_str()
                                                                    .unwrap_or("")
                                                                    .to_string(),
                                                                block_number: log["blockNumber"]
                                                                    .as_str()
                                                                    .unwrap_or("")
                                                                    .to_string(),
                                                                transaction_hash: log
                                                                    ["transactionHash"]
                                                                    .as_str()
                                                                    .unwrap_or("")
                                                                    .to_string(),
                                                                transaction_index: log
                                                                    ["transactionIndex"]
                                                                    .as_str()
                                                                    .unwrap_or("")
                                                                    .to_string(),
                                                                block_hash: log["blockHash"]
                                                                    .as_str()
                                                                    .unwrap_or("")
                                                                    .to_string(),
                                                                log_index: log["logIndex"]
                                                                    .as_str()
                                                                    .unwrap_or("")
                                                                    .to_string(),
                                                                removed: log["removed"]
                                                                    .as_bool()
                                                                    .unwrap_or(false),
                                                                timestamp: block_timestamp,
                                                                decoded_event: None,
                                                            };
                                                            Some(event_log)
                                                        }
                                                    }
                                                })
                                                .collect();

                                            if events.is_empty() && !logs.is_empty() {
                                                warn!("Failed to parse any events from non-empty logs");
                                                HttpResponse::InternalServerError().json(json!({
                                                    "error": "Failed to parse event logs",
                                                    "raw_logs": logs
                                                }))
                                            } else {
                                                debug!(
                                                    "Successfully processed {} events",
                                                    events.len()
                                                );
                                                HttpResponse::Ok().json(events)
                                            }
                                        } else {
                                            debug!("No logs found in transaction receipt");
                                            HttpResponse::Ok().json(Vec::<EventLog>::new())
                                        }
                                    } else {
                                        warn!("No result found in transaction receipt response");
                                        HttpResponse::NotFound().json(json!({
                                            "error": "Transaction not found"
                                        }))
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to fetch transaction events: {}", e);
                                    HttpResponse::InternalServerError().json(json!({
                                        "error": "Failed to fetch transaction events",
                                        "details": e.to_string()
                                    }))
                                }
                            }
                        }
                        None => {
                            warn!("No available nodes for chain {}", chain_id);
                            HttpResponse::ServiceUnavailable().json(json!({
                                "error": "No available nodes for the specified chain"
                            }))
                        }
                    }
                } else {
                    error!("No load balancer found for chain {}", chain_id);
                    HttpResponse::BadRequest().json(json!({"error": "Invalid chain ID"}))
                }
            } else {
                error!("Chain {} not found in configuration", chain_id);
                HttpResponse::BadRequest().json(json!({"error": "Unsupported chain ID"}))
            }
        }
        None => {
            error!("Invalid or missing chain specification");
            HttpResponse::BadRequest().json(json!({
                "error": "Invalid or missing chain specification"
            }))
        }
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
    aurora_testnet,
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

async fn handle_extension_http_request(
    extension_manager: &ExtensionManager,
    extension_name: &str,
    route_path: &str,
    req: &HttpRequest,
    query: &HashMap<String, String>,
    body: &Option<web::Json<Value>>,
) -> Result<HttpResponse> {
    let routes = extension_manager.get_all_routes();
    let extension_routes = routes
        .get(extension_name)
        .ok_or_else(|| anyhow::anyhow!("Extension '{}' not found", extension_name))?;

    let route = extension_routes
        .iter()
        .find(|r| r.path.trim_start_matches('/') == route_path && r.method == req.method().as_str())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No matching route found for extension '{}' with path '{}'",
                extension_name,
                route_path
            )
        })?;

    let mut args = Vec::new();
    let mut env_vars = HashMap::new();

    // Process route parameters
    if let Some(parameters) = &route.parameters {
        for param in parameters {
            let values: Option<Vec<String>> = if param.is_array_type() {
                match param.source {
                    ParameterSource::Body => body.as_ref().and_then(|b| {
                        b.0.get(&param.name).and_then(|v| {
                            v.as_array().map(|arr| {
                                arr.iter()
                                    .map(|elem| {
                                        if elem.is_boolean() {
                                            elem.as_bool().unwrap().to_string()
                                        } else if elem.is_string() {
                                            elem.as_str().unwrap().to_string()
                                        } else {
                                            elem.to_string().trim_matches('"').to_string()
                                        }
                                    })
                                    .collect()
                            })
                        })
                    }),
                    ParameterSource::Header => req
                        .headers()
                        .get(&param.name)
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.split(',').map(str::trim).map(String::from).collect()),
                    ParameterSource::Query => {
                        let query_string = req.query_string();
                        let values: Vec<String> =
                            url::form_urlencoded::parse(query_string.as_bytes())
                                .filter(|(k, _)| k == &param.name)
                                .map(|(_, v)| v.into_owned())
                                .collect();
                        if values.is_empty() {
                            None
                        } else {
                            Some(values)
                        }
                    }
                    ParameterSource::Path => {
                        let path_params = extract_path_params(&route.path, route_path);
                        path_params
                            .get(&param.name)
                            .map(|v| v.split(',').map(str::trim).map(String::from).collect())
                    }
                }
            } else {
                // Handle non-array parameters
                let value = match param.source {
                    ParameterSource::Body => body.as_ref().and_then(|b| {
                        b.0.get(&param.name).and_then(|v| {
                            if v.is_boolean() {
                                Some(v.as_bool().unwrap().to_string())
                            } else if v.is_string() {
                                Some(v.as_str().unwrap().to_string())
                            } else {
                                Some(v.to_string())
                            }
                        })
                    }),
                    ParameterSource::Header => req
                        .headers()
                        .get(&param.name)
                        .and_then(|v| v.to_str().ok().map(String::from)),
                    ParameterSource::Query => query.get(&param.name).cloned(),
                    ParameterSource::Path => {
                        let path_params = extract_path_params(&route.path, route_path);
                        path_params.get(&param.name).cloned()
                    }
                };
                value.map(|v| vec![v])
            };

            if let Some(values) = values {
                match (
                    param.is_array_type(),
                    param.array_style.as_ref().unwrap_or(&ArrayStyle::Repeated),
                ) {
                    (true, ArrayStyle::Variadic) => {
                        // Handle variadic arrays (like --args value1 value2 value3)
                        if !values.is_empty() {
                            args.push(format!("--{}", param.name));
                            args.extend(values);
                        }
                    }
                    (true, ArrayStyle::Repeated) => {
                        // Handle repeated arrays (like --param value1 --param value2)
                        for value in values {
                            args.push(format!("--{}", param.name));
                            args.push(value);
                        }
                    }
                    (true, ArrayStyle::Concatenated) => {
                        // Handle concatenated arrays (like --param value1,value2,value3)
                        if !values.is_empty() {
                            args.push(format!("--{}", param.name));
                            args.push(values.join(","));
                        }
                    }
                    (false, _) => {
                        // Handle non-array types
                        if param.param_type == "boolean" {
                            let value = values.first().unwrap();
                            if value == "true" || value == "1" {
                                args.push(format!("--{}", param.name));
                            }
                        } else {
                            args.push(format!("--{}", param.name));
                            args.push(values.first().unwrap().clone());
                        }
                    }
                }
            } else if param.required {
                return Err(anyhow::anyhow!(
                    "Required parameter '{}' is missing",
                    param.name
                ));
            }
        }
    }

    // Add query parameters that are not in the route definition
    for (key, value) in query {
        if !args.contains(&format!("--{}", key)) {
            args.push(format!("--{}", key));
            args.push(value.clone());
        }
    }

    let config_content = fs::read_to_string(&extension_manager.config_path).await?;
    let doc = config_content.parse::<DocumentMut>()?;
    if let Some(extensions) = doc["extensions"].as_table() {
        if let Some(ext_config) = extensions.get(extension_name) {
            if let Some(ext_table) = ext_config.as_table() {
                for (key, value) in ext_table.iter() {
                    if key != "name" {
                        env_vars.insert(
                            key.to_string(),
                            value.as_str().unwrap_or_default().to_string(),
                        );
                    }
                }
            }
        }
    }

    debug!("Environment variables for extension: {:?}", env_vars);
    debug!("Command arguments: {:?}", args);

    // Execute the extension
    let output = extension_manager
        .run_extension(extension_name, &route.command, args, env_vars)
        .await?;

    Ok(HttpResponse::Ok().body(output))
}

async fn handle_extension_request(
    data: web::Data<AppState>,
    req: HttpRequest,
    query: web::Query<HashMap<String, String>>,
    body: Option<web::Json<Value>>,
) -> ActixResult<HttpResponse> {
    let path = req.uri().path();
    info!("Received request for path: {}", path);
    debug!("Request method: {}", req.method());
    debug!("Query parameters: {:?}", query);
    debug!("Request body: {:?}", body);

    let path_segments: Vec<&str> = path.split('/').collect();
    if path_segments.len() < 4 {
        error!("Invalid path: {}", path);
        return Ok(HttpResponse::BadRequest().body("Invalid extension request path"));
    }

    let extension_name = path_segments[2];
    let route_path = path_segments[3..].join("/");

    info!(
        "Handling extension request for '{}' with path '{}'",
        extension_name, route_path
    );

    match handle_extension_http_request(
        &data.extension_state.manager,
        extension_name,
        &route_path,
        &req,
        &query.into_inner(),
        &body,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(e) => {
            error!("Extension execution failed: {}", e);
            Ok(HttpResponse::InternalServerError()
                .body(format!("Extension execution failed: {}", e)))
        }
    }
}

async fn catch_all(req: HttpRequest) -> ActixResult<HttpResponse> {
    info!("Catch-all route hit: {} {}", req.method(), req.uri());
    Ok(HttpResponse::NotFound().body(format!("No route found for {} {}", req.method(), req.uri())))
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
            .route(
                "/tx/{tx_hash}/events",
                web::get().to(get_transaction_events),
            )
            .route(
                "/{chain}/tx/{tx_hash}/events",
                web::get().to(get_transaction_events),
            )
            .service(web::resource("/eth").route(web::post().to(ethereum)))
            .service(web::resource("/aurora-testnet").route(web::post().to(aurora_testnet)))
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
            web::scope("/extensions"),
            |extensions_scope, (extension_name, routes)| {
                info!("Registering routes for extension: {}", extension_name);
                routes.iter().fold(extensions_scope, |scope, route| {
                    let full_path =
                        format!("/{}/{}", extension_name, route.path.trim_start_matches('/'));
                    info!("Registering route: {} {}", route.method, full_path);
                    scope.route(
                        &full_path,
                        match route.method.as_str() {
                            "GET" => web::get().to(handle_extension_request),
                            "POST" => web::post().to(handle_extension_request),
                            "PUT" => web::put().to(handle_extension_request),
                            "DELETE" => web::delete().to(handle_extension_request),
                            _ => {
                                debug!("Unknown method {} for route {}", route.method, full_path);
                                web::route().to(|_req: HttpRequest| async {
                                    HttpResponse::MethodNotAllowed()
                                })
                            }
                        },
                    )
                })
            },
        );

        debug!("Registering extensions_scope to app");

        app.service(extensions_scope)
            .default_service(web::route().to(catch_all))
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
