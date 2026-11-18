use std::net::SocketAddr;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use aion_session_store::{app_router, AppState, StoreDb, TokenVerifier};

#[derive(Parser)]
#[command(name = "aion_session_store")]
#[command(about = "AION Session Store service")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Applies database migrations using AION_SESSION_STORE_MIGRATION_DATABASE_URL (as db owner)
    Migrate,
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    if let Some(Commands::Migrate) = cli.command {
        let migration_url = match std::env::var("AION_SESSION_STORE_MIGRATION_DATABASE_URL") {
            Ok(url) if !url.trim().is_empty() => url,
            _ => {
                eprintln!("AION_SESSION_STORE_MIGRATION_DATABASE_URL is required for migrations");
                return ExitCode::FAILURE;
            }
        };
        tracing::info!("running database migrations...");
        if let Err(err) = aion_session_store::db::migrate(&migration_url).await {
            eprintln!("migration failed: {err}");
            return ExitCode::FAILURE;
        }
        tracing::info!("migrations applied successfully");
        return ExitCode::SUCCESS;
    }

    let database_url = match std::env::var("AION_SESSION_STORE_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => {
            eprintln!("AION_SESSION_STORE_DATABASE_URL is required to run server");
            return ExitCode::FAILURE;
        }
    };

    let jwks_url = std::env::var("AGENT_CONTROL_JWKS_URL").ok();
    let permissive_auth = std::env::var("AION_SESSION_STORE_PERMISSIVE_AUTH")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let db = match StoreDb::connect_lazy(&database_url, 16) {
        Ok(db) => db,
        Err(err) => {
            eprintln!("failed to configure database pool: {err}");
            return ExitCode::FAILURE;
        }
    };

    let verifier = TokenVerifier::new(jwks_url, permissive_auth);
    let state = AppState { db, verifier };
    let app = app_router(state);

    let port: u16 = env_or("PORT", &env_or("AION_SESSION_STORE_PORT", "8980"))
        .parse()
        .unwrap_or(8980);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("aion_session_store listening on {}", addr);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(err) => {
            eprintln!("failed to bind {addr}: {err}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = axum::serve(listener, app).await {
        eprintln!("server error: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
