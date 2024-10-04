use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
pub struct NodeHealth {
    pub url: String,
    pub healthy: bool,
    pub last_check: DateTime<Utc>,
    #[serde(skip)]
    connections: Arc<AtomicUsize>,
}

impl NodeHealth {
    pub fn new(url: String, healthy: bool) -> Self {
        Self {
            url,
            healthy,
            last_check: Utc::now(),
            connections: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn increment_connections(&self) -> usize {
        self.connections.fetch_add(1, Ordering::SeqCst)
    }

    pub fn decrement_connections(&self) -> usize {
        self.connections.fetch_sub(1, Ordering::SeqCst)
    }

    pub fn get_connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub fn update_health(&mut self, healthy: bool) {
        self.healthy = healthy;
        self.last_check = Utc::now();
    }
}

pub struct NodeList {
    pub nodes: Vec<Arc<NodeHealth>>,
}

impl NodeList {
    pub fn new(nodes: Vec<String>) -> Self {
        let nodes = nodes
            .into_iter()
            .map(|url| Arc::new(NodeHealth::new(url, true)))
            .collect();
        Self { nodes }
    }

    pub fn new_with_health(nodes: Vec<NodeHealth>) -> Self {
        let nodes = nodes.into_iter().map(Arc::new).collect();
        Self { nodes }
    }
}
