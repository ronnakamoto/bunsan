use crate::error::Result;
use crate::node::{NodeHealth, NodeList};
use arc_swap::ArcSwap;
use log::{info, warn};
use reqwest::Client;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout};

pub async fn update_node_health(
    nodes: Arc<ArcSwap<NodeList>>,
    client: Client,
    interval: Duration,
) -> Result<()> {
    loop {
        sleep(interval).await;

        let node_urls: Vec<String> = nodes.load().nodes.iter().map(|n| n.url.clone()).collect();
        let mut node_health = Vec::new();

        for url in node_urls {
            let health = check_node_health(&client, &url).await;
            node_health.push(NodeHealth::new(url, health));
        }

        let new_node_list = Arc::new(NodeList::new_with_health(node_health));

        nodes.store(new_node_list);

        info!(
            "Updated node health status. Healthy nodes: {}",
            nodes.load().nodes.iter().filter(|n| n.healthy).count()
        );
    }
}

pub async fn check_node_health(client: &Client, url: &str) -> bool {
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
            sleep(Duration::from_secs(2u64.pow(attempt))).await;
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
