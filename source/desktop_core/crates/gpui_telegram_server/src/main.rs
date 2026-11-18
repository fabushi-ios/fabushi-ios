use gpui_telegram_server::{build_router_with_state, server::AppState, ServerConfig};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    ServerConfig::load_dotenv_files();
    if std::env::var("GPUI_OFFLINE_TEST").is_err()
        && std::env::var("HERMES_ENABLE_PUBLIC_CLOUD_FALLBACK").is_err()
    {
        unsafe {
            std::env::set_var("HERMES_ENABLE_PUBLIC_CLOUD_FALLBACK", "1");
        }
    }

    let config = ServerConfig::from_env();
    info!(
        "Configuration loaded: host={}, port={}, database_url={}",
        config.host, config.port, config.database_url
    );

    let state = match AppState::try_new(config.clone()) {
        Ok(state) => Arc::new(state),
        Err(err) => {
            error!(
                "Fatal storage startup failure for database_url='{}': {}",
                config.database_url, err
            );
            eprintln!(
                "Fatal storage startup failure for database_url='{}': {}",
                config.database_url, err
            );
            return Err(err.into());
        }
    };
    let state_for_shutdown = Arc::clone(&state);
    let app = build_router_with_state(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));

    info!("Starting gpui_telegram_server listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            state_for_shutdown.shutdown();
        })
        .await?;

    info!("gpui_telegram_server process cleanly shut down");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C signal handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received SIGINT (Ctrl+C), starting graceful shutdown");
        },
        _ = terminate => {
            info!("Received SIGTERM, starting graceful shutdown");
        },
    }
}
