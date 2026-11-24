use std::path::PathBuf;
use std::process::Command;

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

/// Registers Ferrum Browser under HKCU (no UAC admin required) so it appears in
/// Windows Settings -> Default Apps for http, https, .htm, .html, and .pdf,
/// then opens the Windows Default Apps settings page.
pub fn register_as_windows_browser() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let exe =
            std::env::current_exe().map_err(|e| format!("No se pudo ubicar el ejecutable: {e}"))?;
        let exe_str = exe.to_string_lossy().replace('\'', "''");

        let ps = format!(
            r#"
$exe = '{exe_str}'
$base = 'HKCU:\Software\Clients\StartMenuInternet\FerrumBrowser'
$cap = "$base\Capabilities"
New-Item -Path "$base\DefaultIcon" -Force | Out-Null
New-Item -Path "$base\shell\open\command" -Force | Out-Null
New-Item -Path "$cap\URLAssociations" -Force | Out-Null
New-Item -Path "$cap\FileAssociations" -Force | Out-Null
New-Item -Path 'HKCU:\Software\RegisteredApplications' -Force | Out-Null
New-Item -Path 'HKCU:\Software\Classes\FerrumHTML\DefaultIcon' -Force | Out-Null
New-Item -Path 'HKCU:\Software\Classes\FerrumHTML\shell\open\command' -Force | Out-Null

Set-ItemProperty -Path $base -Name '(Default)' -Value 'Ferrum Browser'
Set-ItemProperty -Path "$base\DefaultIcon" -Name '(Default)' -Value "$exe,0"
Set-ItemProperty -Path "$base\shell\open\command" -Name '(Default)' -Value "`"$exe`""

Set-ItemProperty -Path $cap -Name 'ApplicationName' -Value 'Ferrum Browser'
Set-ItemProperty -Path $cap -Name 'ApplicationDescription' -Value 'Navegador web minimalista y seguro en Rust'
Set-ItemProperty -Path $cap -Name 'ApplicationIcon' -Value "$exe,0"
Set-ItemProperty -Path "$cap\URLAssociations" -Name 'http' -Value 'FerrumHTML'
Set-ItemProperty -Path "$cap\URLAssociations" -Name 'https' -Value 'FerrumHTML'
Set-ItemProperty -Path "$cap\FileAssociations" -Name '.htm' -Value 'FerrumHTML'
Set-ItemProperty -Path "$cap\FileAssociations" -Name '.html' -Value 'FerrumHTML'
Set-ItemProperty -Path "$cap\FileAssociations" -Name '.pdf' -Value 'FerrumHTML'

Set-ItemProperty -Path 'HKCU:\Software\RegisteredApplications' -Name 'FerrumBrowser' -Value 'Software\Clients\StartMenuInternet\FerrumBrowser\Capabilities'
Set-ItemProperty -Path 'HKCU:\Software\Classes\FerrumHTML' -Name '(Default)' -Value 'Ferrum Web Document'
Set-ItemProperty -Path 'HKCU:\Software\Classes\FerrumHTML\DefaultIcon' -Name '(Default)' -Value "$exe,0"
Set-ItemProperty -Path 'HKCU:\Software\Classes\FerrumHTML\shell\open\command' -Name '(Default)' -Value "`"$exe`" `"%1`""
Start-Process 'ms-settings:defaultapps'
"#
        );

        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &ps,
            ])
            .status()
            .map_err(|e| format!("Error al registrar en Windows: {e}"))?;

        if status.success() {
            Ok(
                "Ferrum registrado en Windows · elige Ferrum en Aplicaciones predeterminadas"
                    .into(),
            )
        } else {
            Err("No se pudo completar el registro en Windows.".into())
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("Registro nativo disponible solo en Windows.".into())
    }
}

/// Creates Start Menu & Desktop shortcuts for Ferrum Browser.
pub fn create_windows_shortcuts() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let exe =
            std::env::current_exe().map_err(|e| format!("No se pudo ubicar el ejecutable: {e}"))?;
        let exe_str = exe.to_string_lossy().replace('\'', "''");
        let work_dir = exe
            .parent()
            .map(|p| p.to_string_lossy().replace('\'', "''"))
            .unwrap_or_default();

        let ps = format!(
            r#"
$ws = New-Object -ComObject WScript.Shell
$desk = [Environment]::GetFolderPath('Desktop')
$start = Join-Path ([Environment]::GetFolderPath('Programs')) 'Ferrum Browser.lnk'
foreach ($dst in @((Join-Path $desk 'Ferrum Browser.lnk'), $start)) {{
    $sc = $ws.CreateShortcut($dst)
    $sc.TargetPath = '{exe_str}'
    $sc.WorkingDirectory = '{work_dir}'
    $sc.IconLocation = '{exe_str},0'
    $sc.Description = 'Ferrum Browser — Navegador web minimalista y seguro en Rust'
    $sc.Save()
}}
"#
        );

        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &ps,
            ])
            .status()
            .map_err(|e| format!("Error al crear accesos directos: {e}"))?;

        if status.success() {
            Ok("Accesos directos creados en Escritorio y Menú Inicio".into())
        } else {
            Err("No se pudieron crear los accesos directos.".into())
        }
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
        let exe_str = exe.to_string_lossy().replace('\'', "''");
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
        let safe_name_ps = app_name.replace('\'', "''");
        let safe_url_ps = url.replace('\'', "''").replace('"', "");

        let ps = format!(
            r#"
$ws = New-Object -ComObject WScript.Shell
$desk = [Environment]::GetFolderPath('Desktop')
$sc = $ws.CreateShortcut((Join-Path $desk '{safe_name_ps}.lnk'))
$sc.TargetPath = '{exe_str}'
$sc.Arguments = '--app="{safe_url_ps}" --app-title="{safe_name_ps}"'
$sc.IconLocation = '{exe_str},0'
$sc.Description = '{safe_name_ps} (Ferrum Web App)'
$sc.Save()
"#
        );

        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &ps,
            ])
            .status()
            .map_err(|e| format!("Error al instalar Web App: {e}"))?;

        if status.success() {
            Ok(format!("Web App «{app_name}» instalada en el Escritorio"))
        } else {
            Err("No se pudo crear el acceso directo de la Web App.".into())
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (title, url);
        Err("Instalación de Web App disponible solo en Windows.".into())
    }
}

/// Checks if Ferrum is currently registered in HKCU RegisteredApplications.
pub fn is_registered_in_windows() -> bool {
    #[cfg(target_os = "windows")]
    {
        Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\RegisteredApplications",
                "/v",
                "FerrumBrowser",
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
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
