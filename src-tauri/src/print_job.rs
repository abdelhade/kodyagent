use crate::printers::{get_default_printer, set_default_printer};
use crate::settings::AgentSettings;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintJob {
    /// receipt | prep | extra | custom
    #[serde(default = "default_job_type")]
    pub r#type: String,
    pub url: String,
    /// اختياري: رقم مجموعة الأصناف لبون التحضير
    #[serde(default)]
    pub group_id: Option<i64>,
    /// تجاوز اختيار الطابعة
    #[serde(default)]
    pub printer: Option<String>,
}

fn default_job_type() -> String {
    "custom".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintRequest {
    pub jobs: Vec<PrintJob>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrintResult {
    pub ok: bool,
    pub printed: usize,
    pub message: String,
}

pub fn resolve_printer(settings: &AgentSettings, job: &PrintJob) -> String {
    if let Some(p) = job.printer.as_ref().filter(|s| !s.trim().is_empty()) {
        return p.clone();
    }
    match job.r#type.as_str() {
        "extra" => settings.extra_printer.clone(),
        "prep" => {
            if let Some(gid) = job.group_id {
                let key = gid.to_string();
                if let Some(p) = settings.group_printers.get(&key) {
                    if !p.trim().is_empty() {
                        return p.clone();
                    }
                }
            }
            settings.main_printer.clone()
        }
        _ => settings.main_printer.clone(),
    }
}

pub async fn run_jobs(app: AppHandle, settings: AgentSettings, req: PrintRequest) -> PrintResult {
    if req.jobs.is_empty() {
        return PrintResult {
            ok: false,
            printed: 0,
            message: "لا توجد مهام طباعة".into(),
        };
    }

    let previous_default = get_default_printer();
    let mut printed = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for job in req.jobs {
        let printer = resolve_printer(&settings, &job);
        if printer.trim().is_empty() {
            // طابعة إضافية غير مضبوطة = تجاهل بدون خطأ
            if job.r#type == "extra" {
                continue;
            }
            errors.push(format!(
                "لا توجد طابعة معيّنة لمهمة {}",
                job.r#type
            ));
            continue;
        }

        match print_to_printer(&app, &job, &printer).await {
            Ok(()) => printed += 1,
            Err(e) => errors.push(e),
        }

        // نسخة إضافية للفاتورة لو مضبوطة ومختلفة عن العامة
        if job.r#type == "receipt" {
            let extra = settings.extra_printer.trim().to_string();
            if !extra.is_empty() && extra != printer {
                match print_to_printer(&app, &job, &extra).await {
                    Ok(()) => printed += 1,
                    Err(e) => errors.push(e),
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(400)).await;
    }

    if let Some(prev) = previous_default {
        let _ = set_default_printer(&prev);
    }

    if printed == 0 {
        PrintResult {
            ok: false,
            printed: 0,
            message: if errors.is_empty() {
                "فشلت الطباعة".into()
            } else {
                errors.join(" | ")
            },
        }
    } else if errors.is_empty() {
        PrintResult {
            ok: true,
            printed,
            message: format!("تمت طباعة {printed} مهمة"),
        }
    } else {
        PrintResult {
            ok: true,
            printed,
            message: format!(
                "تمت طباعة {printed} مهمة مع أخطاء: {}",
                errors.join(" | ")
            ),
        }
    }
}

async fn print_to_printer(app: &AppHandle, job: &PrintJob, printer: &str) -> Result<(), String> {
    set_default_printer(printer)?;
    print_url_once(app, &job.url).await?;
    let _ = app.emit(
        "print-progress",
        serde_json::json!({
            "type": job.r#type,
            "printer": printer,
            "url": job.url,
        }),
    );
    Ok(())
}

async fn print_url_once(app: &AppHandle, url: &str) -> Result<(), String> {
    let label = format!("print-{}", Uuid::new_v4());
    let parsed = url
        .parse()
        .map_err(|e| format!("رابط غير صالح: {e}"))?;

    let win = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(parsed))
        .title("Kody Print")
        .inner_size(420.0, 640.0)
        .visible(false)
        .skip_taskbar(true)
        .build()
        .map_err(|e| format!("تعذر فتح نافذة الطباعة: {e}"))?;

    // انتظار تحميل الصفحة ثم طباعة
    tokio::time::sleep(Duration::from_millis(1800)).await;

    let script = r#"
        (function () {
          try {
            if (typeof window.print === 'function') {
              window.print();
              return 'ok';
            }
            return 'no-print';
          } catch (e) {
            return String(e);
          }
        })()
    "#;

    let _ = win.eval(script);
    tokio::time::sleep(Duration::from_millis(2500)).await;

    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.close();
    }

    Ok(())
}
