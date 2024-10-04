use crate::config::AppConfig;
use crate::connection_pool::{DynamicClientPool, PooledClient};
use crate::load_balancer::{LoadBalancingStrategy, StrategyType};
use crate::node::NodeList;
use crate::server::Chain;
use arc_swap::ArcSwap;
use log::{debug, info, warn};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time;

pub struct BenchmarkResult {
    pub chain: Option<Chain>,
    pub chain_id: u64,
    pub chain_name: String,
    pub strategy: StrategyType,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub total_time: Duration,
    pub requests_per_second: f64,
    pub avg_latency: Duration,
    pub max_latency: Duration,
    pub min_latency: Duration,
}

pub async fn run_benchmark(
    config: &AppConfig,
    duration: Duration,
    requests_per_second: u64,
    client_pool: &DynamicClientPool,
) -> Vec<BenchmarkResult> {
    info!("Starting benchmark run for all chains and strategies");
    let mut results = Vec::new();

    for chain_config in &config.chains {
        info!(
            "Benchmarking chain: {} (ID: {})",
            chain_config.name, chain_config.chain_id
        );
        let strategy = chain_config.load_balancing_strategy.create_strategy();
        let nodes = Arc::new(ArcSwap::from_pointee(NodeList::new(
            chain_config.nodes.clone(),
        )));
        let result = benchmark_strategy(
            chain_config.chain,
            chain_config.chain_id,
            chain_config.name.clone(),
            chain_config.load_balancing_strategy,
            strategy,
            nodes,
            duration,
            requests_per_second,
            client_pool,
        )
        .await;
        results.push(result);
    }

    info!("Benchmark run completed for all chains and strategies");
    results
}

async fn benchmark_strategy(
    chain: Option<Chain>,
    chain_id: u64,
    chain_name: String,
    strategy_type: StrategyType,
    strategy: Box<dyn LoadBalancingStrategy>,
    nodes: Arc<ArcSwap<NodeList>>,
    duration: Duration,
    requests_per_second: u64,
    client_pool: &DynamicClientPool,
) -> BenchmarkResult {
    info!(
        "Starting benchmark for chain: {:?} (ID: {}), strategy: {:?}",
        chain, chain_id, strategy_type
    );

    let start_time = Instant::now();
    let mut total_requests: u64 = 0;
    let mut successful_requests: u64 = 0;
    let mut failed_requests: u64 = 0;
    let mut total_latency = Duration::new(0, 0);
    let mut max_latency = Duration::new(0, 0);
    let mut min_latency = Duration::new(u64::MAX, 0);

    while start_time.elapsed() < duration {
        let request_start = Instant::now();
        total_requests += 1;

        if let Some(node) = strategy.select_node(&nodes.load().nodes) {
            node.increment_connections();
            let result = send_benchmark_request(&node.url, client_pool).await;
            node.decrement_connections();

            let request_duration = request_start.elapsed();
            match result {
                Ok(_) => {
                    successful_requests += 1;
                    total_latency += request_duration;
                    max_latency = max_latency.max(request_duration);
                    min_latency = min_latency.min(request_duration);
                    debug!("Request completed: latency = {:?}", request_duration);
                }
                Err(e) => {
                    failed_requests += 1;
                    warn!("Request failed: {}", e);
                }
            }
        } else {
            warn!("No available nodes for request");
            failed_requests += 1;
        }

        // Wait for the next request
        let wait_time = Duration::from_secs_f64(1.0 / requests_per_second as f64);
        time::sleep(wait_time.saturating_sub(request_start.elapsed())).await;
    }

    let total_time = start_time.elapsed();
    let requests_per_second = total_requests as f64 / total_time.as_secs_f64();
    let avg_latency = if successful_requests > 0 {
        Duration::from_nanos((total_latency.as_nanos() / successful_requests as u128) as u64)
    } else {
        Duration::new(0, 0)
    };

    info!(
        "Benchmark completed for chain: {:?} (ID: {}), strategy: {:?}",
        chain, chain_id, strategy_type
    );
    info!("Total requests: {}", total_requests);
    info!("Successful requests: {}", successful_requests);
    info!("Failed requests: {}", failed_requests);
    info!("Total time: {:?}", total_time);

    BenchmarkResult {
        chain,
        chain_id,
        chain_name,
        strategy: strategy_type,
        total_requests,
        successful_requests,
        failed_requests,
        total_time,
        requests_per_second,
        avg_latency,
        max_latency,
        min_latency,
    }
}

async fn send_benchmark_request(
    url: &str,
    client_pool: &DynamicClientPool,
) -> Result<(), Box<dyn std::error::Error>> {
    let client: PooledClient = client_pool.get().await?;
    let payload = json!({
        "jsonrpc": "2.0",
        "method": "eth_blockNumber",
        "params": [],
        "id": 1
    });

    let response = client.post(url).json(&payload).send().await?;
    if !response.status().is_success() {
        return Err(format!("Request failed with status: {}", response.status()).into());
    }
    Ok(())
}

pub fn print_benchmark_results(results: &[BenchmarkResult]) {
    for result in results {
        info!(
            "Chain: {:?} - {} (ID: {})",
            result.chain, result.chain_name, result.chain_id
        );
        info!("Strategy: {:?}", result.strategy);
        info!("Total requests: {}", result.total_requests);
        info!("Successful requests: {}", result.successful_requests);
        info!("Failed requests: {}", result.failed_requests);
        info!("Total time: {:?}", result.total_time);
        info!("Requests per second: {:.2}", result.requests_per_second);
        info!("Average latency: {:?}", result.avg_latency);
        info!("Max latency: {:?}", result.max_latency);
        info!("Min latency: {:?}", result.min_latency);
        info!("-----------------------");
    }
}
