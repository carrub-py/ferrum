use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Default, Serialize)]
pub struct MemoryStats {
    pub main_process_mb: f64,
    pub webview_processes_mb: f64,
    pub total_mb: f64,
    pub process_count: usize,
}

/// Launches an independent Ferrum browser window (`Ctrl+N` or "Move tab to new window").
pub fn spawn_new_browser_window(initial_url: Option<&str>) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut cmd = Command::new(exe);
    cmd.arg("--new-window");
    if let Some(url) = initial_url
        && !url.is_empty()
    {
        cmd.arg(url);
    }
    cmd.spawn().is_ok()
}

/// Launches a sleek standalone Web App (PWA) window for the given URL.
pub fn spawn_pwa_window(url: &str, title: Option<&str>) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut cmd = Command::new(exe);
    cmd.arg(format!("--app={url}"));
    if let Some(t) = title
        && !t.trim().is_empty()
    {
        cmd.arg(format!("--app-title={}", t.trim()));
    }
    cmd.spawn().is_ok()
}

/// Registers Ferrum Browser under HKCU using native Win32 Registry APIs (zero PowerShell, no UAC)
/// so it appears in Windows Settings -> Default Apps for http, https, .htm, .html, and .pdf,
/// then opens the Windows Default Apps settings page.
pub fn register_as_windows_browser() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let exe =
            std::env::current_exe().map_err(|e| format!("No se pudo ubicar el ejecutable: {e}"))?;
        let exe_str = exe.to_string_lossy().into_owned();
        let icon_val = format!("{exe_str},0");
        let open_cmd = format!("\"{exe_str}\"");
        let open_url_cmd = format!("\"{exe_str}\" \"%1\"");

        const BASE: &str = r"Software\Clients\StartMenuInternet\FerrumBrowser";
        const CAP: &str = r"Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities";
        const URL_ASSOC: &str =
            r"Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities\URLAssociations";
        const FILE_ASSOC: &str =
            r"Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities\FileAssociations";

        reg_set_hkcu_sz(BASE, None, "Ferrum Browser")?;
        reg_set_hkcu_sz(&format!(r"{BASE}\DefaultIcon"), None, &icon_val)?;
        reg_set_hkcu_sz(&format!(r"{BASE}\shell\open\command"), None, &open_cmd)?;

        reg_set_hkcu_sz(CAP, Some("ApplicationName"), "Ferrum Browser")?;
        reg_set_hkcu_sz(
            CAP,
            Some("ApplicationDescription"),
            "Navegador web minimalista y seguro en Rust",
        )?;
        reg_set_hkcu_sz(CAP, Some("ApplicationIcon"), &icon_val)?;

        reg_set_hkcu_sz(URL_ASSOC, Some("http"), "FerrumHTML")?;
        reg_set_hkcu_sz(URL_ASSOC, Some("https"), "FerrumHTML")?;
        reg_set_hkcu_sz(FILE_ASSOC, Some(".htm"), "FerrumHTML")?;
        reg_set_hkcu_sz(FILE_ASSOC, Some(".html"), "FerrumHTML")?;
        reg_set_hkcu_sz(FILE_ASSOC, Some(".pdf"), "FerrumHTML")?;

        reg_set_hkcu_sz(
            r"Software\RegisteredApplications",
            Some("FerrumBrowser"),
            CAP,
        )?;
        reg_set_hkcu_sz(r"Software\Classes\FerrumHTML", None, "Ferrum Web Document")?;
        reg_set_hkcu_sz(r"Software\Classes\FerrumHTML\DefaultIcon", None, &icon_val)?;
        reg_set_hkcu_sz(
            r"Software\Classes\FerrumHTML\shell\open\command",
            None,
            &open_url_cmd,
        )?;

        crate::webview_hooks::open_file_native("ms-settings:defaultapps");

        Ok("Ferrum registrado en Windows · elige Ferrum en Aplicaciones predeterminadas".into())
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("Registro nativo disponible solo en Windows.".into())
    }
}

/// Creates Start Menu & Desktop shortcuts for Ferrum Browser via native Win32 COM `IShellLinkW`.
pub fn create_windows_shortcuts() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let exe =
            std::env::current_exe().map_err(|e| format!("No se pudo ubicar el ejecutable: {e}"))?;
        let exe_str = exe.to_string_lossy().into_owned();
        let work_dir = exe
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let desktop = known_folder_path(true)?;
        let programs = known_folder_path(false)?;

        let desc = "Ferrum Browser — Navegador web minimalista y seguro en Rust";
        create_lnk_native(
            &desktop.join("Ferrum Browser.lnk"),
            &exe_str,
            "",
            &work_dir,
            desc,
        )?;
        create_lnk_native(
            &programs.join("Ferrum Browser.lnk"),
            &exe_str,
            "",
            &work_dir,
            desc,
        )?;

        Ok("Accesos directos creados en Escritorio y Menú Inicio".into())
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("Accesos directos disponibles solo en Windows.".into())
    }
}

/// Creates a Desktop Web App (.lnk) shortcut that opens a website directly in Ferrum PWA mode.
pub fn create_pwa_desktop_shortcut(title: &str, url: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let exe =
            std::env::current_exe().map_err(|e| format!("No se pudo ubicar el ejecutable: {e}"))?;
        let exe_str = exe.to_string_lossy().into_owned();
        let work_dir = exe
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let safe_name: String = title
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
            .take(40)
            .collect::<String>()
            .trim()
            .to_string();
        let app_name = if safe_name.is_empty() {
            "Ferrum App".to_string()
        } else {
            safe_name
        };
        let safe_url = url.replace('"', "");
        let args = format!("--app=\"{safe_url}\" --app-title=\"{app_name}\"");
        let desc = format!("{app_name} (Ferrum Web App)");

        let desktop = known_folder_path(true)?;
        create_lnk_native(
            &desktop.join(format!("{app_name}.lnk")),
            &exe_str,
            &args,
            &work_dir,
            &desc,
        )?;

        Ok(format!("Web App «{app_name}» instalada en el Escritorio"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (title, url);
        Err("Instalación de Web App disponible solo en Windows.".into())
    }
}

/// Checks if Ferrum is currently registered in HKCU RegisteredApplications using native Win32 Registry API.
pub fn is_registered_in_windows() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_READ, REG_VALUE_TYPE, RegCloseKey, RegOpenKeyExW,
            RegQueryValueExW,
        };
        use windows_core::w;

        unsafe {
            let mut hkey = HKEY::default();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                w!(r"Software\RegisteredApplications"),
                Some(0),
                KEY_READ,
                &mut hkey,
            )
            .is_err()
            {
                return false;
            }
            let mut val_type = REG_VALUE_TYPE::default();
            let mut size = 0u32;
            let exists = RegQueryValueExW(
                hkey,
                w!("FerrumBrowser"),
                None,
                Some(&mut val_type),
                None,
                Some(&mut size),
            )
            .is_ok();
            let _ = RegCloseKey(hkey);
            exists
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Samples live RAM consumption (Working Set) of Ferrum Browser and its descendant WebView2 processes.
pub fn sample_browser_memory() -> MemoryStats {
    #[cfg(target_os = "windows")]
    {
        use std::collections::{HashMap, HashSet};
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };
        use windows::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows::Win32::System::Threading::{
            GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
        };

        unsafe {
            let self_pid = GetCurrentProcessId();
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return MemoryStats::default();
            };

            let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snap, &mut entry).is_ok() {
                loop {
                    children
                        .entry(entry.th32ParentProcessID)
                        .or_default()
                        .push(entry.th32ProcessID);
                    if Process32NextW(snap, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);

            // BFS all descendant PIDs of our Ferrum process (includes WebView2 browser/renderer/GPU processes)
            let mut visited = HashSet::new();
            let mut queue = vec![self_pid];
            visited.insert(self_pid);
            while let Some(pid) = queue.pop() {
                if let Some(kids) = children.get(&pid) {
                    for &k in kids {
                        if visited.insert(k) {
                            queue.push(k);
                        }
                    }
                }
            }

            let read_ws_bytes = |pid: u32| -> u64 {
                let Ok(handle) = OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
                    false,
                    pid,
                ) else {
                    return 0;
                };
                let mut pmc = PROCESS_MEMORY_COUNTERS {
                    cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                    ..Default::default()
                };
                let bytes = if GetProcessMemoryInfo(handle, &mut pmc, pmc.cb).is_ok() {
                    pmc.WorkingSetSize as u64
                } else {
                    0
                };
                let _ = CloseHandle(handle);
                bytes
            };

            let main_bytes = read_ws_bytes(self_pid);
            let mut child_bytes = 0u64;
            for &pid in &visited {
                if pid != self_pid {
                    child_bytes = child_bytes.saturating_add(read_ws_bytes(pid));
                }
            }

            let main_mb = (main_bytes as f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0;
            let web_mb = (child_bytes as f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0;
            MemoryStats {
                main_process_mb: main_mb,
                webview_processes_mb: web_mb,
                total_mb: ((main_mb + web_mb) * 10.0).round() / 10.0,
                process_count: visited.len(),
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        MemoryStats::default()
    }
}

/// Default folder for encrypted `.ferrum-backup` exports.
pub fn backups_dir() -> PathBuf {
    let dir = dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Ferrum Respaldos");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(target_os = "windows")]
fn reg_set_hkcu_sz(subkey: &str, value_name: Option<&str>, data: &str) -> Result<(), String> {
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };
    use windows_core::{HSTRING, PCWSTR};

    let subkey_h = HSTRING::from(subkey);
    let name_h = value_name.map(HSTRING::from);
    let wide: Vec<u16> = data.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };

    unsafe {
        let mut hkey = HKEY::default();
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &subkey_h,
            Some(0),
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .ok()
        .map_err(|e| format!("Error creando clave de registro {subkey}: {e}"))?;

        let name_pcw = name_h
            .as_ref()
            .map(|h| PCWSTR::from_raw(h.as_ptr()))
            .unwrap_or(PCWSTR::null());
        let res = RegSetValueExW(hkey, name_pcw, Some(0), REG_SZ, Some(bytes));
        let _ = RegCloseKey(hkey);
        res.ok()
            .map_err(|e| format!("Error escribiendo valor en {subkey}: {e}"))
    }
}

#[cfg(target_os = "windows")]
fn known_folder_path(desktop: bool) -> Result<PathBuf, String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Programs, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };

    let id = if desktop {
        &FOLDERID_Desktop
    } else {
        &FOLDERID_Programs
    };
    unsafe {
        let raw = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)
            .map_err(|e| format!("No se pudo ubicar la carpeta de Windows: {e}"))?;
        let s = raw
            .to_string()
            .map_err(|e| format!("Ruta de carpeta inválida: {e}"))?;
        CoTaskMemFree(Some(raw.0 as _));
        Ok(PathBuf::from(s))
    }
}

#[cfg(target_os = "windows")]
fn create_lnk_native(
    lnk_path: &std::path::Path,
    target_exe: &str,
    arguments: &str,
    work_dir: &str,
    description: &str,
) -> Result<(), String> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize, IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows_core::{HSTRING, Interface, PCWSTR};

    unsafe {
        let init_hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let did_init = init_hr.is_ok();

        let res = (|| -> Result<(), String> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| format!("No se pudo crear IShellLinkW: {e}"))?;

            let target_h = HSTRING::from(target_exe);
            let args_h = HSTRING::from(arguments);
            let dir_h = HSTRING::from(work_dir);
            let desc_h = HSTRING::from(description);

            link.SetPath(&target_h)
                .map_err(|e| format!("Error asignando ruta de acceso directo: {e}"))?;
            if !arguments.is_empty() {
                let _ = link.SetArguments(&args_h);
            }
            if !work_dir.is_empty() {
                let _ = link.SetWorkingDirectory(&dir_h);
            }
            let _ = link.SetIconLocation(&target_h, 0);
            let _ = link.SetDescription(&desc_h);

            let persist: IPersistFile = link
                .cast()
                .map_err(|e| format!("Error obteniendo IPersistFile: {e}"))?;
            let lnk_h = HSTRING::from(lnk_path.to_string_lossy().as_ref());
            persist
                .Save(PCWSTR::from_raw(lnk_h.as_ptr()), true)
                .map_err(|e| format!("No se pudo guardar el acceso directo: {e}"))?;
            Ok(())
        })();

        if did_init {
            CoUninitialize();
        }
        res
    }
}
