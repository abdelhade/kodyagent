use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
#[link(name = "winspool")]
unsafe extern "system" {
    fn SetDefaultPrinterW(psz_printer: *const u16) -> i32;
    fn GetDefaultPrinterW(psz_buffer: *mut u16, pcch_buffer: *mut u32) -> i32;
}

fn powershell() -> Command {
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass"]);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// قائمة أسماء الطابعات المثبتة على الجهاز (Windows).
pub fn list_printers() -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
        let output = powershell()
            .args([
                "-Command",
                "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; Get-Printer | Select-Object -ExpandProperty Name",
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
        unsafe {
            let mut needed: u32 = 0;
            GetDefaultPrinterW(std::ptr::null_mut(), &mut needed);
            if needed == 0 {
                return None;
            }
            let mut buf = vec![0u16; needed as usize];
            let ok = GetDefaultPrinterW(buf.as_mut_ptr(), &mut needed);
            if ok == 0 {
                return None;
            }
            let nul = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let name = String::from_utf16_lossy(&buf[..nul]).trim().to_string();
            if name.is_empty() {
                None
            } else {
                Some(name)
            }
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
        if let Some(cur) = get_default_printer() {
            if cur.eq_ignore_ascii_case(name) {
                return Ok(());
            }
        }
        let wide: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let ok = unsafe { SetDefaultPrinterW(wide.as_ptr()) };
        if ok == 0 {
            return Err(format!("فشل تعيين الطابعة الافتراضية «{name}»"));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Ok(())
    }
}

fn candidate_sumatra_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let names = [
        "SumatraPDF.exe",
        "SumatraPDF-3.5.2-64.exe",
        "SumatraPDF-3.5.2-32.exe",
    ];

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for n in names {
                out.push(dir.join(n));
                out.push(dir.join("tools").join(n));
            }
            let agent_root = dir
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .and_then(|p| p.parent());
            if let Some(root) = agent_root {
                for n in names {
                    out.push(root.join("tools").join(n));
                }
            }
        }
    }

    let manifest_tools = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tools");
    for n in names {
        out.push(manifest_tools.join(n));
    }

    out
}

fn find_sumatra() -> Option<PathBuf> {
    for p in candidate_sumatra_paths() {
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn print_with_sumatra(sumatra: &Path, pdf_path: &str, printer: &str) -> Result<(), String> {
    let mut cmd = Command::new(sumatra);
    // noscale: التزم بمقاس الـ PDF الحراري (fit كان يصغّر الفاتورة جداً)
    cmd.args([
        "-print-to",
        printer,
        "-silent",
        "-print-settings",
        "noscale",
        pdf_path,
    ]);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd
        .output()
        .map_err(|e| format!("تعذر تشغيل SumatraPDF: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "SumatraPDF فشل الطباعة على «{printer}»: {err} {out}"
        ));
    }

    // مهلة قصيرة للطابعة الحرارية
    thread::sleep(Duration::from_millis(400));
    Ok(())
}

/// طباعة ملف PDF مباشرة على طابعة بالاسم (بدون dialog) — مسار احتياطي.
pub fn print_pdf_to_printer(pdf_path: &str, printer: &str) -> Result<(), String> {
    if printer.trim().is_empty() {
        return Err("اسم الطابعة فاضي".into());
    }
    if !Path::new(pdf_path).is_file() {
        return Err(format!("ملف PDF غير موجود: {pdf_path}"));
    }

    #[cfg(windows)]
    {
        if let Some(sumatra) = find_sumatra() {
            return print_with_sumatra(&sumatra, pdf_path, printer);
        }
        Err("SumatraPDF.exe غير موجود بجانب الوكيل (tools/)".into())
    }

    #[cfg(not(windows))]
    {
        let _ = (pdf_path, printer);
        Err("طباعة PDF غير مدعومة على هذا النظام".into())
    }
}
