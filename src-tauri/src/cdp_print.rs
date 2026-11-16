//! طباعة صامتة عبر Edge/Chrome headless + CDP، مع احتياطي CLI.
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use once_cell::sync::Lazy;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const CDP_PORT: u16 = 19292;
/// عرض ورق حراري 80mm بالبوصة
const PAPER_WIDTH_IN: f64 = 80.0 / 25.4;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct BrowserProc {
    child: Child,
    profile_dir: PathBuf,
}

struct CdpState {
    browser: Option<BrowserProc>,
}

static STATE: Lazy<Mutex<CdpState>> = Lazy::new(|| Mutex::new(CdpState { browser: None }));

fn find_browser() -> Result<PathBuf, String> {
    let candidates = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err("لم يتم العثور على Edge أو Chrome".into())
}

fn profile_dir() -> PathBuf {
    std::env::temp_dir().join("kody-print-agent-cdp")
}

async fn port_open(port: u16) -> bool {
    tokio::time::timeout(
        Duration::from_millis(300),
        TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .is_some()
}

async fn wait_port(port: u16, tries: u32) -> bool {
    for _ in 0..tries {
        if port_open(port).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn kill_browser(state: &mut CdpState) {
    if let Some(mut proc) = state.browser.take() {
        let _ = proc.child.kill();
        let _ = proc.child.wait();
        let _ = fs::remove_dir_all(&proc.profile_dir);
    }
}

async fn ensure_browser_locked(state: &mut CdpState) -> Result<(), String> {
    if let Some(proc) = state.browser.as_mut() {
        match proc.child.try_wait() {
            Ok(None) => {
                if port_open(CDP_PORT).await {
                    return Ok(());
                }
            }
            _ => {
                kill_browser(state);
            }
        }
    }

    if port_open(CDP_PORT).await {
        // منفذ مفتوح من عملية خارجية — نستخدمه
        return Ok(());
    }

    let browser = find_browser()?;
    let profile = profile_dir();
    let _ = fs::remove_dir_all(&profile);
    fs::create_dir_all(&profile).map_err(|e| format!("تعذر إنشاء profile: {e}"))?;

    let mut cmd = Command::new(&browser);
    cmd.args([
        "--headless=new",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-extensions",
        "--disable-background-networking",
        "--disable-sync",
        "--disable-translate",
        "--metrics-recording-only",
        "--mute-audio",
        "--hide-scrollbars",
        "--run-all-compositor-stages-before-draw",
        "--window-size=302,900",
        &format!("--remote-debugging-port={CDP_PORT}"),
        &format!("--user-data-dir={}", profile.display()),
        "about:blank",
    ]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!("تعذر تشغيل المتصفح الصامت: {e}"))?;

    if !wait_port(CDP_PORT, 50).await {
        let mut child = child;
        let _ = child.kill();
        let _ = fs::remove_dir_all(&profile);
        return Err("المتصفح الصامت لم يفتح منفذ CDP".into());
    }

    state.browser = Some(BrowserProc {
        child,
        profile_dir: profile,
    });
    Ok(())
}

/// سخّن المتصفح الصامت عند بدء الوكيل.
pub async fn warm_up() {
    let mut state = STATE.lock().await;
    let _ = ensure_browser_locked(&mut state).await;
}

async fn http_get_json(url: &str) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("CDP HTTP: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("CDP HTTP status {}", resp.status()));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("CDP JSON: {e}"))
}

async fn connect_ws(url: &str) -> Result<WsStream, String> {
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| format!("CDP WS: {e}"))?;
    Ok(ws)
}

async fn cdp_call(ws: &mut WsStream, id: u64, method: &str, params: Value) -> Result<Value, String> {
    let msg = serde_json::json!({
        "id": id,
        "method": method,
        "params": params,
    });
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .map_err(|e| format!("CDP send: {e}"))?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err(format!("انتهت مهلة CDP لـ {method}"));
        }
        let next = tokio::time::timeout(left, ws.next())
            .await
            .map_err(|_| format!("انتهت مهلة CDP لـ {method}"))?
            .ok_or_else(|| "CDP WS أغلق".to_string())?
            .map_err(|e| format!("CDP WS read: {e}"))?;

        let text = match next {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
            Message::Ping(p) => {
                let _ = ws.send(Message::Pong(p)).await;
                continue;
            }
            Message::Close(_) => return Err("CDP WS أغلق".into()),
            _ => continue,
        };

        let v: Value = serde_json::from_str(&text).map_err(|e| format!("CDP parse: {e}"))?;
        if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
            if let Some(err) = v.get("error") {
                return Err(format!("CDP {method}: {err}"));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

async fn wait_load_event(ws: &mut WsStream, timeout_ms: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return;
        }
        let next = tokio::time::timeout(left, ws.next()).await;
        let Ok(Some(Ok(msg))) = next else {
            return;
        };
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Ping(p) => {
                let _ = ws.send(Message::Pong(p)).await;
                continue;
            }
            _ => continue,
        };
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if v.get("method").and_then(|m| m.as_str()) == Some("Page.loadEventFired") {
                return;
            }
        }
    }
}

fn with_no_auto(url: &str) -> String {
    if url.contains("no_auto=") {
        return url.to_string();
    }
    if url.contains('?') {
        format!("{url}&no_auto=1")
    } else {
        format!("{url}?no_auto=1")
    }
}

fn write_pdf_bytes(bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.len() < 500 {
        return Err("PDF الناتج صغير جداً".into());
    }
    let dir = std::env::temp_dir().join("kody-print-agent");
    fs::create_dir_all(&dir).map_err(|e| format!("مجلد مؤقت: {e}"))?;
    let pdf_path = dir.join(format!("job-{}.pdf", Uuid::new_v4()));
    fs::write(&pdf_path, bytes).map_err(|e| format!("كتابة PDF: {e}"))?;
    Ok(pdf_path)
}

async fn close_target(target_id: &str) {
    if target_id.is_empty() {
        return;
    }
    let tid = target_id.to_string();
    if let Ok(c) = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
    {
        let _ = c
            .get(format!("http://127.0.0.1:{CDP_PORT}/json/close/{tid}"))
            .send()
            .await;
    }
}

/// افتح تبويب فاضي فقط — الرابط الحقيقي يتبعت عبر Page.navigate عشان query string متتقطعش.
async fn open_blank_tab() -> Result<(String, String), String> {
    let page_info = match http_get_json(&format!(
        "http://127.0.0.1:{CDP_PORT}/json/new?about:blank"
    ))
    .await
    {
        Ok(v) => v,
        Err(_) => {
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|e| e.to_string())?;
            let resp = client
                .put(format!("http://127.0.0.1:{CDP_PORT}/json/new?about:blank"))
                .send()
                .await
                .map_err(|e| format!("CDP new tab: {e}"))?;
            resp.json::<Value>()
                .await
                .map_err(|e| format!("CDP new tab JSON: {e}"))?
        }
    };
    let ws_url = page_info
        .get("webSocketDebuggerUrl")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "لا يوجد webSocketDebuggerUrl للتبويب".to_string())?
        .to_string();
    let target_id = page_info
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok((target_id, ws_url))
}

async fn print_pdf_via_cdp(url: &str, cookies: Option<&str>) -> Result<PathBuf, String> {
    let (target_id, ws_url) = open_blank_tab().await?;

    let mut ws = connect_ws(&ws_url).await?;
    let mut next_id = 1u64;

    let _ = cdp_call(&mut ws, next_id, "Page.enable", serde_json::json!({})).await?;
    next_id += 1;
    let _ = cdp_call(&mut ws, next_id, "Runtime.enable", serde_json::json!({})).await?;
    next_id += 1;
    let _ = cdp_call(&mut ws, next_id, "Network.enable", serde_json::json!({})).await?;
    next_id += 1;

    // جلسة المستخدم من الـ POS — عشان متتحوّلش لصفحة اللوجين
    if let Some(c) = cookies.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let _ = cdp_call(
            &mut ws,
            next_id,
            "Network.setExtraHTTPHeaders",
            serde_json::json!({ "headers": { "Cookie": c } }),
        )
        .await;
        next_id += 1;
    }

    // مهم: شيل أي device metrics — بتسبب Printing failed على Edge
    let _ = cdp_call(
        &mut ws,
        next_id,
        "Emulation.clearDeviceMetricsOverride",
        serde_json::json!({}),
    )
    .await;
    next_id += 1;

    // التنقل هنا بالـ JSON يحافظ على agent_token كامل
    let _ = cdp_call(
        &mut ws,
        next_id,
        "Page.navigate",
        serde_json::json!({ "url": url }),
    )
    .await?;
    next_id += 1;
    wait_load_event(&mut ws, 12000).await;

    // استنى جاهزية المستند + خطوط
    let _ = cdp_call(
        &mut ws,
        next_id,
        "Runtime.evaluate",
        serde_json::json!({
            "expression": "document.fonts && document.fonts.ready ? document.fonts.ready.then(()=>true) : true",
            "awaitPromise": true,
            "returnByValue": true
        }),
    )
    .await;
    next_id += 1;
    tokio::time::sleep(Duration::from_millis(350)).await;

    // امنع طباعة اللوجين/الرئيسية
    let guard = cdp_call(
        &mut ws,
        next_id,
        "Runtime.evaluate",
        serde_json::json!({
            "expression": "(() => { const href = location.href || ''; const title = (document.title || '') + ' '; const body = ((document.body && document.body.innerText) || '').slice(0, 800); const hasPass = !!document.querySelector('input[type=password]'); const hasPrinted = !!document.getElementById('printed'); const isPrintPage = /\\/(receipt|preparation|combo)\\.php/i.test(href); const isLocalDoc = /^file:/i.test(href) && hasPrinted; const errText = /file not found|couldn't be found|cannot find|تعذر العثور|الملف غير موجود/i.test(title + body + href); const bad = hasPass || errText || (!hasPrinted && !isPrintPage && !isLocalDoc); return { href, hasPrinted, hasPass, errText, bad, title: document.title || '' }; })()",
            "returnByValue": true
        }),
    )
    .await
    .unwrap_or(Value::Null);
    next_id += 1;
    let bad = guard
        .pointer("/result/value/bad")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let href = guard
        .pointer("/result/value/href")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if bad {
        let _ = ws.close(None).await;
        close_target(&target_id).await;
        return Err(format!(
            "رفض الطباعة: الصفحة مش فاتورة/تحضير (لوجين؟). url={href}"
        ));
    }

    let height_eval = cdp_call(
        &mut ws,
        next_id,
        "Runtime.evaluate",
        serde_json::json!({
            "expression": "(() => { const el = document.getElementById('printed') || document.body; return Math.ceil(Math.max(el.scrollHeight||0, document.documentElement.scrollHeight||0, 400)); })()",
            "returnByValue": true
        }),
    )
    .await
    .unwrap_or(Value::Null);
    next_id += 1;

    let height_px = height_eval
        .pointer("/result/value")
        .and_then(|v| v.as_f64())
        .unwrap_or(900.0)
        .max(300.0);
    let paper_height_in = ((height_px / 96.0) + 0.25).clamp(2.0, 20.0);

    // ثبّت عرض 80mm صراحة (متعتمدش على CSS وحدها عشان متطلعش A4)
    let attempts = [
        serde_json::json!({
            "landscape": false,
            "displayHeaderFooter": false,
            "printBackground": true,
            "preferCSSPageSize": false,
            "paperWidth": PAPER_WIDTH_IN,
            "paperHeight": paper_height_in,
            "marginTop": 0, "marginBottom": 0, "marginLeft": 0, "marginRight": 0,
            "scale": 1
        }),
        serde_json::json!({
            "landscape": false,
            "displayHeaderFooter": false,
            "printBackground": true,
            "preferCSSPageSize": true,
            "paperWidth": PAPER_WIDTH_IN,
            "paperHeight": paper_height_in,
            "marginTop": 0, "marginBottom": 0, "marginLeft": 0, "marginRight": 0,
            "scale": 1
        }),
    ];

    let mut last_err = String::from("Printing failed");
    let mut pdf_b64: Option<String> = None;
    for params in attempts {
        match cdp_call(&mut ws, next_id, "Page.printToPDF", params).await {
            Ok(result) => {
                if let Some(data) = result.get("data").and_then(|v| v.as_str()) {
                    pdf_b64 = Some(data.to_string());
                    break;
                }
                last_err = "CDP لم يُرجع بيانات PDF".into();
            }
            Err(e) => last_err = e,
        }
        next_id += 1;
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    let _ = ws.close(None).await;
    close_target(&target_id).await;

    let b64 = pdf_b64.ok_or(last_err)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("فشل فك PDF: {e}"))?;
    write_pdf_bytes(&bytes)
}

fn file_url_for(path: &std::path::Path) -> Result<String, String> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("cwd: {e}"))?
            .join(path)
    };
    let mut s = abs.to_string_lossy().to_string();
    // Windows canonicalize أحياناً بيرجع \\?\C:\...
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        s = rest.to_string();
    }
    s = s.replace('\\', "/");
    if s.chars().nth(1) == Some(':') {
        Ok(format!("file:///{s}"))
    } else if s.starts_with('/') {
        Ok(format!("file://{s}"))
    } else {
        Err(format!("مسار HTML غير صالح: {s}"))
    }
}

/// HTML جاهز من كودي → ملف مؤقت → PDF حراري 80mm.
pub async fn html_to_thermal_pdf(html: &str) -> Result<PathBuf, String> {
    if html.trim().len() < 50 {
        return Err("HTML فاضي".into());
    }

    let dir = std::env::temp_dir().join("kody-print-agent");
    fs::create_dir_all(&dir).map_err(|e| format!("مجلد مؤقت: {e}"))?;
    let html_path = dir.join(format!("doc-{}.html", Uuid::new_v4()));
    fs::write(&html_path, html.as_bytes()).map_err(|e| format!("كتابة HTML: {e}"))?;
    let file_url = file_url_for(&html_path)?;

    let mut state = STATE.lock().await;
    ensure_browser_locked(&mut state).await?;

    let result = print_pdf_via_cdp(&file_url, None).await;
    let _ = fs::remove_file(&html_path);

    match result {
        Ok(path) => Ok(path),
        Err(first_err) => {
            kill_browser(&mut state);
            ensure_browser_locked(&mut state).await?;
            // أعد كتابة الملف للمحاولة التانية
            fs::write(&html_path, html.as_bytes()).map_err(|e| format!("كتابة HTML: {e}"))?;
            let file_url = file_url_for(&html_path)?;
            let retry = print_pdf_via_cdp(&file_url, None).await;
            let _ = fs::remove_file(&html_path);
            retry.map_err(|e| format!("{first_err} | إعادة: {e}"))
        }
    }
}

/// مسار قديم: فتح URL (احتياطي).
pub async fn url_to_thermal_pdf(url: &str, cookies: Option<&str>) -> Result<PathBuf, String> {
    let url = with_no_auto(url);

    let mut state = STATE.lock().await;
    ensure_browser_locked(&mut state).await?;

    match print_pdf_via_cdp(&url, cookies).await {
        Ok(path) => Ok(path),
        Err(first_err) => {
            kill_browser(&mut state);
            ensure_browser_locked(&mut state).await?;
            print_pdf_via_cdp(&url, cookies)
                .await
                .map_err(|e| format!("{first_err} | إعادة: {e}"))
        }
    }
}
