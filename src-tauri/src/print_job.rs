use crate::cdp_print;
use crate::printers::{get_default_printer, print_pdf_to_printer, set_default_printer};
use crate::settings::AgentSettings;
use serde::{Deserialize, Serialize};
use std::fs;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintJob {
    /// receipt | prep | extra | custom
    #[serde(default = "default_job_type")]
    pub r#type: String,
    /// قديم: رابط صفحة — يُستخدم فقط لو مفيش html
    #[serde(default)]
    pub url: String,
    /// HTML جاهز من السيرفر (المسار الأساسي)
    #[serde(default)]
    pub html: Option<String>,
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
    #[serde(default)]
    pub cookies: Option<String>,
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
    let mut last_printer = String::new();
    let cookies = req
        .cookies
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    for job in req.jobs {
        let printer = resolve_printer(&settings, &job);
        if printer.trim().is_empty() {
            if job.r#type == "extra" {
                continue;
            }
            errors.push(format!("لا توجد طابعة معيّنة لمهمة {}", job.r#type));
            continue;
        }

        if !printer.eq_ignore_ascii_case(&last_printer) {
            if let Err(e) = set_default_printer(&printer) {
                errors.push(e);
                continue;
            }
            last_printer = printer.clone();
        }

        match print_to_printer(&app, &job, &printer, cookies.as_deref()).await {
            Ok(()) => printed += 1,
            Err(e) => errors.push(e),
        }

        if job.r#type == "receipt" {
            let extra = settings.extra_printer.trim().to_string();
            if !extra.is_empty() && !extra.eq_ignore_ascii_case(&printer) {
                if let Err(e) = set_default_printer(&extra) {
                    errors.push(e);
                } else {
                    last_printer = extra.clone();
                    match print_to_printer(&app, &job, &extra, cookies.as_deref()).await {
                        Ok(()) => printed += 1,
                        Err(e) => errors.push(e),
                    }
                }
            }
        }
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

async fn print_to_printer(
    app: &AppHandle,
    job: &PrintJob,
    printer: &str,
    cookies: Option<&str>,
) -> Result<(), String> {
    let pdf_path = if let Some(html) = job.html.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty())
    {
        // المسار الأساسي: HTML جاهز من كودي → PDF 80mm → الطابعة
        cdp_print::html_to_thermal_pdf(html).await?
    } else if !job.url.trim().is_empty() {
        cdp_print::url_to_thermal_pdf(job.url.as_str(), cookies).await?
    } else {
        return Err("مهمة الطباعة فاضية (لا html ولا url)".into());
    };

    let pdf_str = pdf_path
        .to_str()
        .ok_or_else(|| "مسار PDF غير صالح".to_string())?
        .to_string();

    let print_result = print_pdf_to_printer(&pdf_str, printer);
    let _ = fs::remove_file(&pdf_path);
    print_result?;

    let _ = app.emit(
        "print-progress",
        serde_json::json!({
            "type": job.r#type,
            "printer": printer,
        }),
    );

    tokio::time::sleep(Duration::from_millis(80)).await;
    Ok(())
}
