use crate::connection_pool::{DynamicClientPool, PooledClient};
use crate::error::Result;
use crate::node::{NodeHealth, NodeList};
use arc_swap::ArcSwap;
use log::warn;
use serde_json::json;
use std::sync::Arc;
use tokio::time::Duration;

pub async fn check_node_health(client: &PooledClient, url: &str) -> bool {
    let health_check_payload = json!({
        "jsonrpc": "2.0",
        "method": "web3_clientVersion",
        "params": [],
        "id": 1
    });

    match client
        .post(url)
        .json(&health_check_payload)
        .timeout(Duration::from_secs(5))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => true,
        Ok(response) => {
            warn!(
                "Node {} responded with non-success status: {}",
                url,
                response.status()
            );
            false
        }
        Err(e) => {
            warn!("Failed to check health for node {}: {}", url, e);
            false
        }
    }
}

pub async fn update_node_health(
    nodes: &Arc<ArcSwap<NodeList>>,
    client_pool: &DynamicClientPool,
) -> Result<()> {
    let current_nodes = nodes.load();
    let mut updated_nodes = Vec::new();

    for node in current_nodes.nodes.iter() {
        let client = client_pool.get().await?;
        let health = check_node_health(&client, &node.url).await;
        updated_nodes.push(NodeHealth::new(node.url.clone(), health));
    }

    nodes.store(Arc::new(NodeList::new_with_health(updated_nodes)));
    Ok(())
}
