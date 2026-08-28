use std::sync::Arc;
use tokio::sync::Notify;
use tracing::info;

mod chain;
mod config;
mod content;
mod http;
mod merkle;
mod reconcile;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lichen_moss_provider=info,tower_http=info".into()),
        )
        .init();

    let config = Arc::new(config::Config::from_env().unwrap_or_else(|error| {
        eprintln!("Moss provider configuration error: {error}");
        std::process::exit(78);
    }));
    let store = Arc::new(
        content::ContentStore::open(
            config.data_dir.clone(),
            config.max_object_bytes,
            config.max_total_bytes,
        )
        .await
        .unwrap_or_else(|error| {
            eprintln!("Moss provider storage error: {error}");
            std::process::exit(78);
        }),
    );
    let chain = Arc::new(
        chain::ChainClient::load(&config.rpc_url, config.contract, &config.keypair_path)
            .unwrap_or_else(|error| {
                eprintln!("Moss provider chain error: {error}");
                std::process::exit(78);
            }),
    );
    let notify = Arc::new(Notify::new());
    let state = http::AppState::new(config.clone(), store.clone(), chain.clone(), notify.clone());
    let app = http::build_app(state).unwrap_or_else(|error| {
        eprintln!("Moss provider HTTP configuration error: {error}");
        std::process::exit(78);
    });

    tokio::spawn(reconcile::run(
        config.clone(),
        store.clone(),
        chain.clone(),
        notify,
    ));

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .unwrap_or_else(|error| {
            eprintln!("Moss provider listen error: {error}");
            std::process::exit(1);
        });
    info!(
        listen = %config.listen,
        provider = %chain.provider().to_base58(),
        stored_bytes = store.stored_bytes(),
        loopback = config.is_loopback(),
        "Lichen Moss provider ready"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap_or_else(|error| {
            eprintln!("Moss provider server error: {error}");
            std::process::exit(1);
        });
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl-C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
