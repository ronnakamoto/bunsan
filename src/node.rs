use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Serialize, Deserialize)]
pub struct NodeHealth {
    pub url: String,
    pub healthy: bool,
    pub last_check: DateTime<Utc>,
}

impl NodeHealth {
    pub fn new(url: String, healthy: bool) -> Self {
        Self {
            url,
            healthy,
            last_check: Utc::now(),
        }
    }
}

pub struct NodeList {
    pub nodes: Vec<NodeHealth>,
    index: AtomicUsize,
}

impl NodeList {
    pub fn new(nodes: Vec<String>) -> Self {
        let nodes = nodes
            .into_iter()
            .map(|url| NodeHealth::new(url, true))
            .collect();
        Self {
            nodes,
            index: AtomicUsize::new(0),
        }
    }

    pub fn new_with_health(nodes: Vec<NodeHealth>) -> Self {
        Self {
            nodes,
            index: AtomicUsize::new(0),
        }
    }

    pub fn get_next_node(&self) -> Option<&str> {
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
