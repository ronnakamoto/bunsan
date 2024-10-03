use crate::node::NodeHealth;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::sync::Arc;

pub trait LoadBalancingStrategy: Send + Sync + Any {
    fn select_node(&self, nodes: &[Arc<NodeHealth>]) -> Option<Arc<NodeHealth>>;
    fn as_any(&self) -> &dyn Any;
}

pub struct RoundRobin {
    next: std::sync::atomic::AtomicUsize,
}

impl RoundRobin {
    pub fn new() -> Self {
        Self {
            next: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl LoadBalancingStrategy for RoundRobin {
    fn select_node(&self, nodes: &[Arc<NodeHealth>]) -> Option<Arc<NodeHealth>> {
        let healthy_nodes: Vec<_> = nodes.iter().filter(|n| n.healthy).collect();
        if healthy_nodes.is_empty() {
            return None;
        }
        let index =
            self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst) % healthy_nodes.len();
        Some(Arc::clone(healthy_nodes[index]))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct LeastConnections;

impl LoadBalancingStrategy for LeastConnections {
    fn select_node(&self, nodes: &[Arc<NodeHealth>]) -> Option<Arc<NodeHealth>> {
        nodes
            .iter()
            .filter(|n| n.healthy)
            .min_by_key(|n| n.get_connections())
            .cloned()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct RandomSelection;

impl LoadBalancingStrategy for RandomSelection {
    fn select_node(&self, nodes: &[Arc<NodeHealth>]) -> Option<Arc<NodeHealth>> {
        use rand::seq::SliceRandom;
        let healthy_nodes: Vec<_> = nodes.iter().filter(|n| n.healthy).collect();
        healthy_nodes
            .choose(&mut rand::thread_rng())
            .cloned()
            .map(Arc::clone)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum StrategyType {
    RoundRobin,
    LeastConnections,
    Random,
}

impl StrategyType {
    pub fn create_strategy(&self) -> Box<dyn LoadBalancingStrategy> {
        match self {
            StrategyType::RoundRobin => Box::new(RoundRobin::new()),
            StrategyType::LeastConnections => Box::new(LeastConnections),
            StrategyType::Random => Box::new(RandomSelection),
        }
    }
}

impl From<Box<dyn LoadBalancingStrategy>> for StrategyType {
    fn from(strategy: Box<dyn LoadBalancingStrategy>) -> Self {
        if strategy.as_any().is::<RoundRobin>() {
            StrategyType::RoundRobin
        } else if strategy.as_any().is::<LeastConnections>() {
            StrategyType::LeastConnections
        } else if strategy.as_any().is::<RandomSelection>() {
            StrategyType::Random
        } else {
            panic!("Unknown strategy type")
        }
    }
}
