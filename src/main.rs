mod benchmark;
mod config;
mod error;
mod extensions;
mod health;
mod load_balancer;
mod node;
mod server;

use crate::benchmark::{print_benchmark_results, run_benchmark};
use crate::config::{create_client, watch_config_file, AppConfig, ChainNodeList};
use crate::error::Result;
use crate::extensions::manager::ExtensionManager;
use crate::health::check_node_health;
use crate::node::NodeList;
use crate::server::run_server;

use arc_swap::ArcSwap;
use clap::{Parser, Subcommand};
use colored::*;
use directories::ProjectDirs;
use env_logger::{self, Env};
use log::{debug, error, info, warn};
use prettytable::{Cell, Row, Table};
use server::EventLog;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::Duration;
use toml_edit::DocumentMut;

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
    Tx {
        #[arg(required = true)]
        hash: String,
        #[arg(short = 'n', long)]
        chain: Option<String>,
        #[arg(short, long)]
        fields: Option<String>,
    },
    InstallExtension {
        #[arg(required = true)]
        name: String,
    },
    ListExtensions,
    UninstallExtension {
        #[arg(required = true)]
        name: String,
    },
    RunExtension {
        #[arg(required = true)]
        name: String,
        #[arg(required = true)]
        command: String,
        #[arg(last = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Events {
        #[arg(required = true)]
        hash: String,
        #[arg(short = 'n', long)]
        chain: Option<String>,
    },
}

const DEFAULT_CONFIG: &str = include_str!("../config.toml");

async fn get_transaction(
    config_path: &PathBuf,
    chain: Option<String>,
    hash: String,
    fields: Option<String>,
) -> Result<()> {
    let app_config = AppConfig::load(config_path)?;
    let client = create_client(&app_config.connection_pool);

    let url = format!("http://{}/tx/{}", app_config.server_addr, hash);
    let mut request = client.get(&url);

    if let Some(chain) = chain {
        request = request.header("X-Chain-ID", chain);
    }

    if let Some(fields) = fields {
        request = request.query(&[("fields", fields)]);
    }

    let response = request.send().await?;

    if response.status().is_success() {
        let json: serde_json::Value = response.json().await?;
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        let error: serde_json::Value = response.json().await?;
        eprintln!("Error: {}", error);
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    env_logger::Builder::from_env(Env::default().default_filter_or(&cli.log_level)).init();

    let config_path = cli.config.unwrap_or_else(|| {
        ProjectDirs::from("com", "bunsan", "loadbalancer")
            .map(|proj_dirs| proj_dirs.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("config.toml"))
    });

    let mut extension_manager = ExtensionManager::new(
        &ProjectDirs::from("com", "bunsan", "loadbalancer")
            .map(|proj_dirs| proj_dirs.data_local_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".")),
        &config_path,
    );

    match &cli.command {
        Some(Commands::Start) => start_server(&config_path, &mut extension_manager).await?,
        Some(Commands::Health) => check_health(&config_path).await?,
        Some(Commands::Config { json }) => show_config(&config_path, *json)?,
        Some(Commands::Validate) => validate_config(&config_path)?,
        Some(Commands::Nodes) => list_nodes(&config_path).await?,
        Some(Commands::DeleteConfig) => delete_config(&config_path)?,
        Some(Commands::Benchmark {
            duration,
            requests_per_second,
        }) => run_benchmarks(&config_path, *duration, *requests_per_second).await?,
        Some(Commands::Tx {
            chain,
            hash,
            fields,
        }) => get_transaction(&config_path, chain.clone(), hash.clone(), fields.clone()).await?,
        Some(Commands::InstallExtension { name }) => {
            extension_manager.install_extension(name).await?
        }
        Some(Commands::ListExtensions) => {
            let extensions = extension_manager.list_installed_extensions().await?;
            for ext in extensions {
                println!(
                    "{} (v{}): {}",
                    ext.name,
                    ext.version,
                    ext.description.unwrap_or_default()
                );
            }
        }
        Some(Commands::UninstallExtension { name }) => {
            extension_manager.uninstall_extension(name).await?
        }
        Some(Commands::RunExtension {
            name,
            command,
            args,
        }) => {
            // Clone args to create an owned Vec<String>
            let final_args = args.to_vec();
            let mut env_vars = HashMap::new();

            // Load extension-specific environment variables from config.toml
            let config_content = fs::read_to_string(&config_path)?;
            let doc = config_content.parse::<DocumentMut>()?;
            if let Some(extensions) = doc["extensions"].as_table() {
                if let Some(ext_config) = extensions.get(name) {
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

            match extension_manager
                .run_extension(name, command, final_args, env_vars)
                .await
            {
                Ok(result) => println!("{}", result),
                Err(e) => {
                    eprintln!("Error running extension: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Events { chain, hash }) => {
            get_transaction_events(&config_path, chain.clone(), hash.clone()).await?
        }
        None => start_server(&config_path, &mut extension_manager).await?,
    }

    Ok(())
}

async fn start_server(
    config_path: &PathBuf,
    extension_manager: &mut ExtensionManager,
) -> Result<()> {
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

    // Create the client with connection pooling
    let client = create_client(&app_config.connection_pool);

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

    // Load extensions
    extension_manager.load_extensions().await?;

    // Create ExtensionState
    let extension_state = Arc::new(server::ExtensionState {
        manager: Arc::new(extension_manager.clone()),
        routes: extension_manager.get_all_routes().clone(),
    });

    // Run the server
    run_server(app_config, chain_nodes, client, extension_state).await?;

    Ok(())
}

async fn check_health(config_path: &PathBuf) -> Result<()> {
    let app_config = AppConfig::load(config_path)?;
    let client = create_client(&app_config.connection_pool);

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
        println!("Connection pool:");
        println!("  Min idle: {:?}", app_config.connection_pool.min_idle);
        println!("  Max size: {}", app_config.connection_pool.max_size);
        println!(
            "  Idle timeout: {:?} seconds",
            app_config.connection_pool.idle_timeout
        );
        println!(
            "  Connection timeout: {} ms",
            app_config.connection_pool.connection_timeout
        );
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
    let client = create_client(&app_config.connection_pool);
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

async fn get_transaction_events(
    config_path: &PathBuf,
    chain: Option<String>,
    hash: String,
) -> Result<()> {
    let app_config = AppConfig::load(config_path)?;
    let client = create_client(&app_config.connection_pool);

    let url = format!("http://{}/tx/{}/events", app_config.server_addr, hash);
    let mut request = client.get(&url);

    if let Some(chain) = chain {
        request = request.header("X-Chain-ID", chain);
    }

    let response = request.send().await?;

    if response.status().is_success() {
        let response_text = response.text().await?;
        debug!("Raw response: {}", response_text);

        let events: Vec<EventLog> = serde_json::from_str(&response_text).map_err(|e| {
            error!("Failed to parse events JSON: {}", e);
            error!("Response text: {}", response_text);
            anyhow::anyhow!("Failed to parse events JSON: {}", e)
        })?;

        if events.is_empty() {
            println!("No events found for transaction {}", hash);
        } else {
            println!("\nFound {} events for transaction {}:", events.len(), hash);
            for (i, event) in events.iter().enumerate() {
                println!("\nEvent #{}", i + 1);

                // Only print non-empty fields
                if !event.address.is_empty() {
                    println!("  Address: {}", event.address);
                }
                if !event.block_number.is_empty() {
                    println!("  Block Number: {}", event.block_number);
                }
                if !event.transaction_index.is_empty() {
                    println!("  Transaction Index: {}", event.transaction_index);
                }
                if !event.log_index.is_empty() {
                    println!("  Log Index: {}", event.log_index);
                }

                if !event.topics.is_empty() {
                    println!("  Topics:");
                    for (j, topic) in event.topics.iter().enumerate() {
                        println!("    {}: {}", j, topic);
                    }
                }

                if !event.data.is_empty() {
                    println!("  Data: {}", event.data);
                }

                if let Some(timestamp) = &event.timestamp {
                    println!("  Timestamp: {}", timestamp);
                }

                if let Some(decoded) = &event.decoded_event {
                    println!("  Decoded Event:");
                    println!("    Name: {}", decoded.name);
                    println!("    Parameters:");
                    for param in &decoded.params {
                        println!(
                            "      {}{}: {}",
                            param.name,
                            if param.indexed { " (indexed)" } else { "" },
                            param.value
                        );
                    }
                }
                println!("  {}", "-".repeat(50));
            }
        }
    } else {
        let error_text = response.text().await?;
        eprintln!("Error response: {}", error_text);
        return Err(anyhow::anyhow!("Failed to fetch events: {}", error_text));
    }

    Ok(())
}
