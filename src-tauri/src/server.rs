use crate::print_job::{PrintRequest, PrintResult, run_jobs};
use crate::settings::{self, AgentSettings};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use parking_lot::RwLock;
use std::net::SocketAddr;
use std::sync::Arc;
use tauri::AppHandle;
use tower_http::cors::{Any, CorsLayer};

#[derive(Clone)]
pub struct ServerState {
    pub app: AppHandle,
    pub settings: Arc<RwLock<AgentSettings>>,
}

pub async fn start(app: AppHandle, settings: Arc<RwLock<AgentSettings>>) {
    let port = {
        let s = settings.read();
        s.agent_port
    };
    let state = ServerState { app, settings };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let router = Router::new()
        .route("/health", get(health))
        .route("/settings", get(get_settings_http))
        .route("/print", post(print_http))
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[print-agent] تعذر فتح المنفذ {port}: {e}");
            return;
        }
    };
    eprintln!("[print-agent] listening on http://127.0.0.1:{port}");
    if let Err(e) = axum::serve(listener, router).await {
        eprintln!("[print-agent] server error: {e}");
    }
}

async fn health(State(state): State<ServerState>) -> Json<serde_json::Value> {
    let s = state.settings.read().clone();
    Json(serde_json::json!({
        "ok": true,
        "service": "kody-print-agent",
        "port": s.agent_port,
        "server_url": s.server_url,
        "main_printer": s.main_printer,
        "extra_printer": s.extra_printer,
    }))
}

async fn get_settings_http(State(state): State<ServerState>) -> Json<AgentSettings> {
    Json(state.settings.read().clone())
}

async fn print_http(
    State(state): State<ServerState>,
    Json(req): Json<PrintRequest>,
) -> (StatusCode, Json<PrintResult>) {
    // إعادة تحميل من القرص لو اتعدّلت من الواجهة
    let disk = settings::load();
    *state.settings.write() = disk.clone();

    let result = run_jobs(state.app.clone(), disk, req).await;
    let code = if result.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    (code, Json(result))
}
