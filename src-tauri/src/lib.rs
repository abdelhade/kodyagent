mod print_job;
mod printers;
mod server;
mod settings;

use print_job::{PrintJob, PrintRequest, PrintResult, run_jobs};
use settings::AgentSettings;
use parking_lot::RwLock;
use serde::Serialize;
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, WindowEvent,
};

struct AppState {
    settings: Arc<RwLock<AgentSettings>>,
}

#[derive(Serialize)]
struct TestConnectionResult {
    ok: bool,
    message: String,
    groups: Vec<GroupItem>,
}

#[derive(Serialize, Clone)]
struct GroupItem {
    id: i64,
    name: String,
}

#[tauri::command]
fn get_settings(state: tauri::State<'_, AppState>) -> AgentSettings {
    state.settings.read().clone()
}

#[tauri::command]
fn save_settings(state: tauri::State<'_, AppState>, settings: AgentSettings) -> Result<(), String> {
    settings::save(&settings)?;
    *state.settings.write() = settings;
    Ok(())
}

#[tauri::command]
fn list_printers() -> Result<Vec<String>, String> {
    printers::list_printers()
}

#[tauri::command]
async fn test_connection(server_url: String) -> Result<TestConnectionResult, String> {
    let base = server_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return Ok(TestConnectionResult {
            ok: false,
            message: "اكتب رابط كودي أولاً".into(),
            groups: vec![],
        });
    }

    let ping_url = format!("{base}/api/print_agent.php?action=ping");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;

    let ping_res = client.get(&ping_url).send().await;
    let ping_res = match ping_res {
        Ok(r) => r,
        Err(e) => {
            return Ok(TestConnectionResult {
                ok: false,
                message: format!("فشل الاتصال: {e}"),
                groups: vec![],
            });
        }
    };

    if !ping_res.status().is_success() {
        return Ok(TestConnectionResult {
            ok: false,
            message: format!("السيرفر رد بكود {}", ping_res.status()),
            groups: vec![],
        });
    }

    let ping_json: serde_json::Value = ping_res.json().await.unwrap_or(serde_json::json!({}));
    if ping_json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Ok(TestConnectionResult {
            ok: false,
            message: ping_json
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("رد غير متوقع من كودي")
                .to_string(),
            groups: vec![],
        });
    }

    let groups_url = format!("{base}/api/print_agent.php?action=groups");
    let mut groups = Vec::new();
    if let Ok(gres) = client.get(&groups_url).send().await {
        if let Ok(gjson) = gres.json::<serde_json::Value>().await {
            if let Some(arr) = gjson.get("groups").and_then(|v| v.as_array()) {
                for g in arr {
                    let id = g.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                    let name = g
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if id > 0 && !name.is_empty() {
                        groups.push(GroupItem { id, name });
                    }
                }
            }
        }
    }

    Ok(TestConnectionResult {
        ok: true,
        message: format!("متصل — {} مجموعة", groups.len()),
        groups,
    })
}

#[tauri::command]
async fn print_now(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    jobs: Vec<PrintJob>,
) -> Result<PrintResult, String> {
    let settings = state.settings.read().clone();
    Ok(run_jobs(app, settings, PrintRequest { jobs }).await)
}

#[tauri::command]
fn agent_endpoint(state: tauri::State<'_, AppState>) -> String {
    let port = state.settings.read().agent_port;
    format!("http://127.0.0.1:{port}")
}

fn show_main(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let initial = settings::load();
    let settings = Arc::new(RwLock::new(initial));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            settings: settings.clone(),
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            list_printers,
            test_connection,
            print_now,
            agent_endpoint
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let settings_for_server = settings.clone();
            tauri::async_runtime::spawn(async move {
                server::start(handle, settings_for_server).await;
            });

            let show_i = MenuItem::with_id(app, "show", "فتح الإعدادات", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "إغلاق الوكيل", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Kody Print Agent")
                .icon(app.default_window_icon().unwrap().clone())
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
