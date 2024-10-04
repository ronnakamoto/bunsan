use crate::error::Result;
use crate::node::NodeList;
use arc_swap::ArcSwap;
use log::{info, warn};
use reqwest::Client;
use serde_json::json;
use std::sync::Arc;
use tokio::time::Duration;

pub async fn check_node_health(client: &Client, url: &str) -> bool {
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

pub async fn update_node_health(nodes: &Arc<ArcSwap<NodeList>>, client: &Client) -> Result<()> {
    let current_nodes = nodes.load();
    let mut updated_nodes = Vec::new();

    for node in current_nodes.nodes.iter() {
        let health = check_node_health(client, &node.url).await;
        let mut updated_node = (**node).clone();
        updated_node.update_health(health);
        if health != node.healthy {
            info!("Node {} health status changed to {}", node.url, health);
        }
        updated_nodes.push(Arc::new(updated_node));
    }

    nodes.store(Arc::new(NodeList {
        nodes: updated_nodes,
    }));
    Ok(())
}
