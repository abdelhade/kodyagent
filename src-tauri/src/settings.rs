use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSettings {
    pub server_url: String,
    pub main_printer: String,
    pub extra_printer: String,
    /// group_id (as string) -> printer name
    pub group_printers: HashMap<String, String>,
    pub agent_port: u16,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            server_url: "http://localhost/kody".into(),
            main_printer: String::new(),
            extra_printer: String::new(),
            group_printers: HashMap::new(),
            agent_port: 17890,
        }
    }
}

fn config_path() -> Result<PathBuf, String> {
    let dir = dirs::config_dir()
        .ok_or_else(|| "تعذر تحديد مجلد الإعدادات".to_string())?
        .join("kody-print-agent");
    fs::create_dir_all(&dir).map_err(|e| format!("تعذر إنشاء مجلد الإعدادات: {e}"))?;
    Ok(dir.join("settings.json"))
}

pub fn load() -> AgentSettings {
    let path = match config_path() {
        Ok(p) => p,
        Err(_) => return AgentSettings::default(),
    };
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => AgentSettings::default(),
    }
}

pub fn save(settings: &AgentSettings) -> Result<(), String> {
    let path = config_path()?;
    let raw = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("تعذر تحويل الإعدادات: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("تعذر حفظ الإعدادات: {e}"))
}
