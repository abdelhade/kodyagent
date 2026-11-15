use std::process::Command;

/// قائمة أسماء الطابعات المثبتة على الجهاز (Windows).
pub fn list_printers() -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                "Get-Printer | Select-Object -ExpandProperty Name",
            ])
            .output()
            .map_err(|e| format!("تعذر قراءة الطابعات: {e}"))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(format!("فشل جلب الطابعات: {err}"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut names: Vec<String> = stdout
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    #[cfg(not(windows))]
    {
        Ok(vec!["Default".into()])
    }
}

pub fn get_default_printer() -> Option<String> {
    #[cfg(windows)]
    {
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                "(Get-CimInstance Win32_Printer -Filter \"Default=$true\").Name",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub fn set_default_printer(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        let escaped = name.replace('\'', "''");
        let script = format!(
            "$p = Get-CimInstance Win32_Printer -Filter \"Name='{escaped}'\"; if ($null -eq $p) {{ throw 'printer not found' }}; Invoke-CimMethod -InputObject $p -MethodName SetDefaultPrinter | Out-Null"
        );
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .output()
            .map_err(|e| format!("تعذر تعيين الطابعة الافتراضية: {e}"))?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(format!("فشل تعيين الطابعة «{name}»: {err}"));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Ok(())
    }
}
