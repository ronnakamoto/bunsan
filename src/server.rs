use crate::config::AppConfig;
use crate::error::Result;
use crate::node::NodeList;
use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use arc_swap::ArcSwap;
use log::{error, info};
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct AppState {
    nodes: Arc<ArcSwap<NodeList>>,
    client: Client,
}

async fn health_check(data: web::Data<AppState>) -> impl Responder {
    let nodes = data.nodes.load();
    let healthy_count = nodes.nodes.iter().filter(|n| n.healthy).count();
    let total_count = nodes.nodes.len();
    HttpResponse::Ok().json(json!({
        "status": "ok",
        "healthy_nodes": healthy_count,
        "total_nodes": total_count
    }))
}

async fn rpc_endpoint(body: web::Json<Value>, data: web::Data<AppState>) -> HttpResponse {
    let nodes = data.nodes.load();
    match nodes.get_next_node() {
        Some(evm_node) => {
            let response = data.client.post(evm_node).json(&body).send().await;

            match response {
                Ok(res) => match res.json::<Value>().await {
                    Ok(json) => HttpResponse::Ok().json(json),
                    Err(e) => {
                        error!("Failed to parse response from {}: {}", evm_node, e);
                        HttpResponse::InternalServerError()
                            .json(json!({"error": "Invalid response from node"}))
                    }
                },
                Err(e) => {
                    error!("Failed to forward request to {}: {}", evm_node, e);
                    HttpResponse::InternalServerError()
                        .json(json!({"error": "Failed to forward request"}))
                }
            }
        }
        None => HttpResponse::ServiceUnavailable().json(json!({"error": "No available nodes"})),
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received, starting graceful shutdown");
}

pub async fn run_server(
    config: AppConfig,
    nodes: Arc<ArcSwap<NodeList>>,
    client: Client,
) -> Result<()> {
    let server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(AppState {
                nodes: Arc::clone(&nodes),
                client: client.clone(),
            }))
            .route("/health", web::get().to(health_check))
            .route("/", web::post().to(rpc_endpoint))
    })
    .bind(&config.server_addr)?
    .run();

    tokio::select! {
        result = server => {
            if let Err(e) = result {
                error!("Server error: {}", e);
            }
        },
        _ = shutdown_signal() => {
            info!("Shutting down server");
        },
    }

    Ok(())
}
