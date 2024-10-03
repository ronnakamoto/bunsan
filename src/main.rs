mod config;
mod error;
mod health;
mod load_balancer;
mod node;
mod server;

use crate::config::{watch_config_file, AppConfig};
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
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::Duration;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to the configuration file
    #[arg(short, long, global = true, env = "BUNSAN_CONFIG")]
    config: Option<PathBuf>,

    /// Set log level (error, warn, info, debug, trace)
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
    /// Start the Bunsan server
    Start,
    /// Check the health of all configured nodes
    Health,
    /// Display the current configuration
    Config {
        #[arg(short, long)]
        json: bool,
    },
    /// Validate the configuration file
    Validate,
    /// List all connected nodes and their status
    Nodes,
    /// Delete the configuration file
    DeleteConfig,
}

const DEFAULT_CONFIG: &str = r#"
server_addr = "127.0.0.1:8080"
update_interval = 60
load_balancing_strategy = "LeastConnections"

nodes = [
    "https://1rpc.io/eth",
    "https://eth.drpc.org",
    "https://eth.llamarpc.com",
    "https://ethereum.blockpi.network/v1/rpc/public",
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
    let nodes = Arc::new(ArcSwap::from_pointee(NodeList::new(
        app_config.nodes.clone(),
    )));
    let client = Client::new();

    // Start the file watcher in a background task
    {
        let nodes_clone = Arc::clone(&nodes);
        let client_clone = client.clone();
        let config_path_clone = config_path.clone();

        tokio::spawn(async move {
            if let Err(e) = watch_config_file(&config_path_clone, nodes_clone, client_clone).await {
                error!("File watcher error: {}", e);
            }
        });
    }

    // Start the node health updater
    {
        let nodes_clone = Arc::clone(&nodes);
        let client_clone = client.clone();
        let update_interval = Duration::from_secs(app_config.update_interval);

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(update_interval).await;
                if let Err(e) = crate::health::update_node_health(&nodes_clone, &client_clone).await
                {
                    error!("Update nodes task failed: {}", e);
                }
            }
        });
    }

    run_server(app_config, nodes, client).await?;
    Ok(())
}

async fn check_health(config_path: &PathBuf) -> Result<()> {
    let app_config = AppConfig::load(config_path)?;
    let client = Client::new();

    let mut table = Table::new();
    table.add_row(Row::new(vec![
        Cell::new("Node URL").style_spec("bFc"),
        Cell::new("Status").style_spec("bFc"),
    ]));

    for node in &app_config.nodes {
        let health = check_node_health(&client, node).await;
        let status = if health {
            "Healthy".green()
        } else {
            "Unhealthy".red()
        };
        table.add_row(Row::new(vec![
            Cell::new(node),
            Cell::new(&status.to_string()),
        ]));
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
        println!(
            "Load balancing strategy: {:?}",
            app_config.load_balancing_strategy
        );
        println!("Nodes:");
        for (i, node) in app_config.nodes.iter().enumerate() {
            println!("  {}. {}", i + 1, node);
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
    let nodes = Arc::new(ArcSwap::from_pointee(NodeList::new(
        app_config.nodes.clone(),
    )));

    let mut table = Table::new();
    table.add_row(Row::new(vec![
        Cell::new("Node URL").style_spec("bFc"),
        Cell::new("Health").style_spec("bFc"),
        Cell::new("Connections").style_spec("bFc"),
    ]));

    for node in nodes.load().nodes.iter() {
        let health = check_node_health(&client, &node.url).await;
        let health_status = if health {
            "Healthy".green()
        } else {
            "Unhealthy".red()
        };
        table.add_row(Row::new(vec![
            Cell::new(&node.url),
            Cell::new(&health_status.to_string()),
            Cell::new(&node.get_connections().to_string()),
        ]));
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
