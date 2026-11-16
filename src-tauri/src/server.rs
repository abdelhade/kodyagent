use crate::print_job::{PrintRequest, PrintResult, run_jobs};
use crate::settings::{self, AgentSettings};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::mpsc;
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum JobStatus {
    Queued,
    Printing,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
struct JobInfo {
    id: String,
    status: JobStatus,
    message: String,
    printed: usize,
}

struct QueueItem {
    id: String,
    app: AppHandle,
    settings: AgentSettings,
    req: PrintRequest,
}

#[derive(Clone)]
struct ServerState {
    app: AppHandle,
    settings: Arc<RwLock<AgentSettings>>,
    queue: mpsc::Sender<QueueItem>,
    jobs: Arc<RwLock<HashMap<String, JobInfo>>>,
}

#[derive(Deserialize)]
struct StatusQuery {
    id: String,
}

#[derive(Serialize)]
struct QueueAccept {
    ok: bool,
    job_id: String,
    printed: usize,
    message: String,
}

pub async fn start(app: AppHandle, settings: Arc<RwLock<AgentSettings>>) {
    let port = {
        let s = settings.read();
        s.agent_port
    };

    let jobs: Arc<RwLock<HashMap<String, JobInfo>>> = Arc::new(RwLock::new(HashMap::new()));
    let jobs_worker = jobs.clone();

    let (tx, mut rx) = mpsc::channel::<QueueItem>(64);
    tokio::spawn(async move {
        while let Some(item) = rx.recv().await {
            {
                let mut map = jobs_worker.write();
                if let Some(j) = map.get_mut(&item.id) {
                    j.status = JobStatus::Printing;
                    j.message = "جاري الطباعة".into();
                }
            }

            let result = run_jobs(item.app, item.settings, item.req).await;

            {
                let mut map = jobs_worker.write();
                if let Some(j) = map.get_mut(&item.id) {
                    j.printed = result.printed;
                    j.message = result.message.clone();
                    j.status = if result.ok {
                        JobStatus::Done
                    } else {
                        JobStatus::Failed
                    };
                }
            }

            if result.ok {
                eprintln!("[print-agent] طابور {}: {}", item.id, result.message);
            } else {
                eprintln!("[print-agent] طابور {} فشل: {}", item.id, result.message);
            }
        }
    });

    let state = ServerState {
        app,
        settings,
        queue: tx,
        jobs,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let router = Router::new()
        .route("/health", get(health))
        .route("/settings", get(get_settings_http))
        .route("/print", post(print_http))
        .route("/print/status", get(print_status))
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
        "queue": true,
    }))
}

async fn get_settings_http(State(state): State<ServerState>) -> Json<AgentSettings> {
    Json(state.settings.read().clone())
}

async fn print_status(
    State(state): State<ServerState>,
    Query(q): Query<StatusQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let map = state.jobs.read();
    match map.get(&q.id) {
        Some(info) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "job_id": info.id,
                "status": info.status,
                "message": info.message,
                "printed": info.printed,
            })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "ok": false,
                "message": "مهمة غير موجودة",
            })),
        ),
    }
}

async fn print_http(
    State(state): State<ServerState>,
    Json(req): Json<PrintRequest>,
) -> (StatusCode, Json<QueueAccept>) {
    if req.jobs.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(QueueAccept {
                ok: false,
                job_id: String::new(),
                printed: 0,
                message: "لا توجد مهام طباعة".into(),
            }),
        );
    }

    let disk = settings::load();
    *state.settings.write() = disk.clone();

    let job_id = Uuid::new_v4().to_string();
    let job_count = req.jobs.len();

    state.jobs.write().insert(
        job_id.clone(),
        JobInfo {
            id: job_id.clone(),
            status: JobStatus::Queued,
            message: format!("في الطابور ({job_count})"),
            printed: 0,
        },
    );

    let item = QueueItem {
        id: job_id.clone(),
        app: state.app.clone(),
        settings: disk,
        req,
    };

    match state.queue.send(item).await {
        Ok(()) => (
            StatusCode::OK,
            Json(QueueAccept {
                ok: true,
                job_id,
                printed: 0,
                message: "جاري الطباعة".into(),
            }),
        ),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(QueueAccept {
                ok: false,
                job_id,
                printed: 0,
                message: "طابور الطباعة متوقف".into(),
            }),
        ),
    }
}
