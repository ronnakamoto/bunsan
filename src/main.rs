mod benchmark;
mod config;
mod error;
mod health;
mod load_balancer;
mod node;
mod server;

use crate::benchmark::{print_benchmark_results, run_benchmark};
use crate::config::{watch_config_file, AppConfig, ChainNodeList};
use crate::error::Result;
use crate::health::check_node_health;
use crate::node::NodeList;
use crate::server::run_server;

use arc_swap::ArcSwap;
use clap::{Parser, Subcommand};
use colored::*;
use directories::ProjectDirs;
use env_logger::{self, Env};
use log::{error, info, warn};
use prettytable::{Cell, Row, Table};
use reqwest::Client;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::Duration;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short, long, global = true, env = "BUNSAN_CONFIG")]
    config: Option<PathBuf>,

    #[arg(
        short,
        long,
        global = true,
        env = "BUNSAN_LOG_LEVEL",
        default_value = "info"
    )]
    log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    Start,
    Health,
    Config {
        #[arg(short, long)]
        json: bool,
    },
    Validate,
    Nodes,
    DeleteConfig,
    Benchmark {
        #[arg(short, long, default_value = "60")]
        duration: u64,
        #[arg(short, long, default_value = "100")]
        requests_per_second: u64,
    },
}

const DEFAULT_CONFIG: &str = r#"
server_addr = "127.0.0.1:8080"
update_interval = 60

[[chains]]
name = "Ethereum"
chain_id = 1
chain = "Ethereum"
load_balancing_strategy = "LeastConnections"
nodes = [
    "https://1rpc.io/eth",
    "https://eth.drpc.org",
    "https://eth.llamarpc.com",
    "https://ethereum.blockpi.network/v1/rpc/public",
]

[[chains]]
name = "Optimism"
chain_id = 10
chain = "Optimism"
load_balancing_strategy = "RoundRobin"
nodes = [
    "https://1rpc.io/op",
    "https://optimism.blockpi.network/v1/rpc/public",
]

[[chains]]
name = "Arbitrum One"
chain_id = 42161
chain = "Arbitrum"
load_balancing_strategy = "Random"
nodes = [
    "https://1rpc.io/arb",
    "https://arbitrum.blockpi.network/v1/rpc/public",
]

[[chains]]
name = "BNB Smart Chain Mainnet"
chain_id = 56
chain = "BNBSmartChain"
load_balancing_strategy = "LeastConnections"
nodes = ["https://1rpc.io/bnb", "https://bsc.blockpi.network/v1/rpc/public"]

[[chains]]
name = "BNB Smart Chain Testnet"
chain_id = 97
chain = "BNBSmartChainTestnet"
load_balancing_strategy = "RoundRobin"
nodes = [
    "https://bsc-testnet.blockpi.network/v1/rpc/public",
    "https://data-seed-prebsc-1-s1.bnbchain.org:8545",
    "https://data-seed-prebsc-2-s2.bnbchain.org:8545",
]

[[chains]]
name = "Sepolia Testnet"
chain_id = 11155111
chain = "SepoliaTestnet"
load_balancing_strategy = "LeastConnections"
nodes = [
    "https://rpc.sepolia.org",
    "https://rpc2.sepolia.org",
    "https://1rpc.io/sepolia",
]

[[chains]]
name = "OP Sepolia Testnet"
chain_id = 11155420
chain = "OPSepoliaTestnet"
load_balancing_strategy = "Random"
nodes = [
    "https://sepolia.optimism.io",
    "https://optimism-sepolia.blockpi.network/v1/rpc/public",
    "https://optimism-sepolia.drpc.org",
]

[[chains]]
name = "Arbitrum Sepolia Testnet"
chain_id = 421614
chain = "ArbitrumSepoliaTestnet"
load_balancing_strategy = "RoundRobin"
nodes = [
    "https://sepolia-rollup.arbitrum.io/rpc",
    "https://arbitrum-sepolia.blockpi.network/v1/rpc/public",
]

[[chains]]
name = "Base Sepolia Testnet"
chain_id = 84532
chain = "BaseSepoliaTestnet"
load_balancing_strategy = "LeastConnections"
nodes = [
    "https://sepolia.base.org",
    "https://base-sepolia.blockpi.network/v1/rpc/public",
    "https://base-sepolia-rpc.publicnode.com",
]

[[chains]]
name = "Base"
chain_id = 8453
chain = "Base"
load_balancing_strategy = "RoundRobin"
nodes = [
    "https://mainnet.base.org",
    "https://base.meowrpc.com",
    "https://base.llamarpc.com",
    "https://base.blockpi.network/v1/rpc/public",
    "https://1rpc.io/base",
    "https://developer-access-mainnet.base.org",
]

[[chains]]
name = "Polygon Mainnet"
chain_id = 137
chain = "PolygonMainnet"
load_balancing_strategy = "LeastConnections"
nodes = [
    "https://polygon.meowrpc.com",
    "https://polygon.llamarpc.com",
    "https://polygon.blockpi.network/v1/rpc/public",
]

[[chains]]
name = "Polygon zkEVM"
chain_id = 1101
chain = "PolygonZkEVM"
load_balancing_strategy = "Random"
nodes = [
    "https://1rpc.io/polygon/zkevm",
    "https://rpc.ankr.com/polygon_zkevm",
    "https://zkevm-rpc.com",
]

[[chains]]
name = "Polygon Amoy"
chain_id = 80002
chain = "PolygonAmoy"
load_balancing_strategy = "RoundRobin"
nodes = [
    "https://rpc-amoy.polygon.technology",
    "https://polygon-amoy-bor-rpc.publicnode.com",
    "https://polygon-amoy.drpc.org",
    "https://rpc.ankr.com/polygon_amoy",
]

[[chains]]
name = "Polygon zkEVM Testnet"
chain_id = 1442
chain = "PolygonZkEVMTestnet"
load_balancing_strategy = "LeastConnections"
nodes = [
    "https://rpc.public.zkevm-test.net",
    "https://polygon-zkevm-testnet.drpc.org",
]

[[chains]]
name = "Scroll"
chain_id = 534352
chain = "Scroll"
load_balancing_strategy = "Random"
nodes = [
    "https://scroll.drpc.org",
    "https://scroll-mainnet.chainstacklabs.com",
    "https://scroll-mainnet.public.blastapi.io",
    "https://rpc.ankr.com/scroll",
]

[[chains]]
name = "Scroll Sepolia Testnet"
chain_id = 534351
chain = "ScrollSepoliaTestnet"
load_balancing_strategy = "RoundRobin"
nodes = [
    "https://scroll-sepolia-rpc.publicnode.com",
    "https://scroll-sepolia.blockpi.network/v1/rpc/public",
    "https://sepolia-rpc.scroll.io",
]

[[chains]]
name = "Taiko Mainnet"
chain_id = 167000
chain = "TaikoMainnet"
load_balancing_strategy = "LeastConnections"
nodes = [
    "https://taiko-rpc.publicnode.com",
    "https://rpc.ankr.com/taiko",
    "https://taiko.blockpi.network/v1/rpc/public",
    "https://taiko.drpc.org",
]

[[chains]]
name = "Neon EVM Mainnet"
chain_id = 245022934
chain = "NeonEVMMainnet"
load_balancing_strategy = "Random"
nodes = [
    "https://neon-proxy-mainnet.solana.p2p.org",
    "https://neon-evm.drpc.org",
]

[[chains]]
name = "Neon EVM Devnet"
chain_id = 245022926
chain = "NeonEVMDevnet"
load_balancing_strategy = "RoundRobin"
nodes = [
    "https://devnet.neonevm.org",
    "https://neon-evm-devnet.drpc.org",
]
"#;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    env_logger::Builder::from_env(Env::default().default_filter_or(&cli.log_level)).init();

    let config_path = cli.config.unwrap_or_else(|| {
        ProjectDirs::from("com", "bunsan", "loadbalancer")
            .map(|proj_dirs| proj_dirs.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("config.toml"))
    });

    match &cli.command {
        Some(Commands::Start) => start_server(&config_path).await?,
        Some(Commands::Health) => check_health(&config_path).await?,
        Some(Commands::Config { json }) => show_config(&config_path, *json)?,
        Some(Commands::Validate) => validate_config(&config_path)?,
        Some(Commands::Nodes) => list_nodes(&config_path).await?,
        Some(Commands::DeleteConfig) => delete_config(&config_path)?,
        Some(Commands::Benchmark {
            duration,
            requests_per_second,
        }) => run_benchmarks(&config_path, *duration, *requests_per_second).await?,
        None => start_server(&config_path).await?,
    }

    Ok(())
}

async fn start_server(config_path: &PathBuf) -> Result<()> {
    info!("Starting Bunsan RPC Loadbalancer");
    info!("Using config file: {}", config_path.display());

    if !config_path.exists() {
        warn!("Config file not found. Creating a default config.");
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(config_path, DEFAULT_CONFIG)?;
        info!("Default config created at: {}", config_path.display());
    }

    let app_config = AppConfig::load(config_path)?;
    let chain_nodes = Arc::new(ArcSwap::new(Arc::new(ChainNodeList::new())));
    let client = Client::new();

    // Initialize chain_nodes
    {
        let mut initial_chain_nodes = ChainNodeList::new();
        for chain in &app_config.chains {
            let nodes = NodeList::new(chain.nodes.clone());
            initial_chain_nodes.insert(chain.chain_id, Arc::new(ArcSwap::new(Arc::new(nodes))));
        }
        chain_nodes.store(Arc::new(initial_chain_nodes));
    }

    // Start the file watcher in a background task
    {
        let chain_nodes_clone = Arc::clone(&chain_nodes);
        let client_clone = client.clone();
        let config_path_clone = config_path.clone();

        tokio::spawn(async move {
            if let Err(e) =
                watch_config_file(&config_path_clone, chain_nodes_clone, client_clone).await
            {
                error!("File watcher error: {}", e);
            }
        });
    }

    // Start the node health updater
    {
        let chain_nodes_clone = Arc::clone(&chain_nodes);
        let client_clone = client.clone();
        let update_interval = Duration::from_secs(app_config.update_interval);

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(update_interval).await;
                let current_chain_nodes = chain_nodes_clone.load();
                for nodes in current_chain_nodes.values() {
                    if let Err(e) = crate::health::update_node_health(nodes, &client_clone).await {
                        error!("Update nodes task failed: {}", e);
                    }
                }
            }
        });
    }

    run_server(app_config, chain_nodes, client).await?;
    Ok(())
}

async fn check_health(config_path: &PathBuf) -> Result<()> {
    let app_config = AppConfig::load(config_path)?;
    let client = Client::new();

    let mut table = Table::new();
    table.add_row(Row::new(vec![
        Cell::new("Chain").style_spec("bFc"),
        Cell::new("Chain ID").style_spec("bFc"),
        Cell::new("Node URL").style_spec("bFc"),
        Cell::new("Status").style_spec("bFc"),
    ]));

    for chain in &app_config.chains {
        for node in &chain.nodes {
            let health = check_node_health(&client, node).await;
            let status = if health {
                "Healthy".green()
            } else {
                "Unhealthy".red()
            };
            table.add_row(Row::new(vec![
                Cell::new(&format!("{:?}", chain.chain)),
                Cell::new(&chain.chain_id.to_string()),
                Cell::new(node),
                Cell::new(&status.to_string()),
            ]));
        }
    }

    table.printstd();
    Ok(())
}

fn show_config(config_path: &PathBuf, json: bool) -> Result<()> {
    let app_config = AppConfig::load(config_path)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&app_config)?);
    } else {
        println!("{}", "Current configuration:".bold());
        println!("Server address: {}", app_config.server_addr);
        println!("Update interval: {} seconds", app_config.update_interval);
        println!("Chains:");
        for chain in &app_config.chains {
            println!(
                "  Chain: {:?} - {} (ID: {})",
                chain.chain, chain.name, chain.chain_id
            );
            println!(
                "    Load balancing strategy: {:?}",
                chain.load_balancing_strategy
            );
            println!("    Nodes:");
            for (i, node) in chain.nodes.iter().enumerate() {
                println!("      {}. {}", i + 1, node);
            }
        }
    }
    Ok(())
}

fn validate_config(config_path: &PathBuf) -> Result<()> {
    match AppConfig::load(config_path) {
        Ok(_) => {
            println!("{}", "Configuration is valid.".green());
            Ok(())
        }
        Err(e) => {
            eprintln!("{}", "Configuration is invalid:".red());
            eprintln!("{}", e);
            Err(e)
        }
    }
}

async fn list_nodes(config_path: &PathBuf) -> Result<()> {
    let app_config = AppConfig::load(config_path)?;
    let client = Client::new();
    let mut chain_nodes = HashMap::new();

    for chain in &app_config.chains {
        let nodes = Arc::new(ArcSwap::from_pointee(NodeList::new(chain.nodes.clone())));
        chain_nodes.insert(chain.chain_id, nodes);
    }

    let mut table = Table::new();
    table.add_row(Row::new(vec![
        Cell::new("Chain").style_spec("bFc"),
        Cell::new("Chain ID").style_spec("bFc"),
        Cell::new("Node URL").style_spec("bFc"),
        Cell::new("Health").style_spec("bFc"),
        Cell::new("Connections").style_spec("bFc"),
    ]));

    for chain in &app_config.chains {
        if let Some(nodes) = chain_nodes.get(&chain.chain_id) {
            for node in nodes.load().nodes.iter() {
                let health = check_node_health(&client, &node.url).await;
                let health_status = if health {
                    "Healthy".green()
                } else {
                    "Unhealthy".red()
                };
                table.add_row(Row::new(vec![
                    Cell::new(&format!("{:?}", chain.chain)),
                    Cell::new(&chain.chain_id.to_string()),
                    Cell::new(&node.url),
                    Cell::new(&health_status.to_string()),
                    Cell::new(&node.get_connections().to_string()),
                ]));
            }
        }
    }

    table.printstd();
    Ok(())
}

fn delete_config(config_path: &PathBuf) -> Result<()> {
    if config_path.exists() {
        fs::remove_file(config_path)?;
        println!("{}", "Configuration file deleted:".green());
        println!("{}", config_path.display());
    } else {
        println!("{}", "Configuration file does not exist:".yellow());
        println!("{}", config_path.display());
    }
    Ok(())
}

async fn run_benchmarks(
    config_path: &PathBuf,
    duration: u64,
    requests_per_second: u64,
) -> Result<()> {
    info!("Loading configuration from: {}", config_path.display());
    let app_config = AppConfig::load(config_path)?;
    let benchmark_duration = Duration::from_secs(duration);

    info!("Starting benchmarks...");
    info!("Duration: {} seconds", duration);
    info!("Requests per second: {}", requests_per_second);

    let results = run_benchmark(&app_config, benchmark_duration, requests_per_second).await;
    info!("Benchmark completed. Printing results:");
    print_benchmark_results(&results);

    Ok(())
}
