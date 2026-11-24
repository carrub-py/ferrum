use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const GITHUB_OWNER: &str = "carrub-py";
pub const GITHUB_REPO: &str = "ferrum";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const RELEASES_PAGE_URL: &str = "https://github.com/carrub-py/ferrum/releases";
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/carrub-py/ferrum/releases/latest";
const TARGET_ASSET_NAME: &str = "ferrum-browser.exe";
const CHECKSUMS_ASSET_NAME: &str = "SHA256SUMS.txt";
const MAX_UPDATE_BYTES: u64 = 150 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub tag: String,
    pub html_url: String,
    pub notes: String,
    pub exe_download_url: String,
    pub sha256_url: String,
    pub exe_size: u64,
}

#[derive(Debug, Clone)]
pub enum UpdateCheckResult {
    Available(UpdateInfo),
    UpToDate,
    NoReleasesYet,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// Removes leftover `.old` / `.update` staging binaries left from a previous self-update.
pub fn cleanup_old_binary_on_startup() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let old_path = PathBuf::from(format!("{}.old", exe.to_string_lossy()));
    let upd_path = PathBuf::from(format!("{}.update", exe.to_string_lossy()));
    let _ = fs::remove_file(old_path);
    let _ = fs::remove_file(upd_path);
}

fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(format!(
            "FerrumBrowser/{CURRENT_VERSION} (+https://github.com/{GITHUB_OWNER}/{GITHUB_REPO})"
        ))
        .build()
        .into()
}

fn download_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent(format!(
            "FerrumBrowser/{CURRENT_VERSION} (+https://github.com/{GITHUB_OWNER}/{GITHUB_REPO})"
        ))
        .build()
        .into()
}

pub fn is_trusted_github_asset_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://github.com/carrub-py/ferrum/releases/download/")
        || lower.starts_with("https://objects.githubusercontent.com/")
        || lower.starts_with("https://release-assets.githubusercontent.com/")
}

pub fn parse_semver(raw: &str) -> Option<(u64, u64, u64)> {
    let cleaned = raw.trim().trim_start_matches(['v', 'V']);
    let core = cleaned.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    Some((major, minor, patch))
}

pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    match (parse_semver(current), parse_semver(candidate)) {
        (Some(cur), Some(cand)) => cand > cur,
        _ => false,
    }
}

/// Parses standard `SHA256SUMS.txt` lines (`<64-hex>  filename` or `<64-hex> *filename`).
pub fn parse_sha256sums(content: &str, filename: &str) -> Option<String> {
    for raw_line in content.lines() {
        let line = raw_line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let rest = parts.next()?.trim_start_matches('*');
        let base_name = rest.rsplit(['/', '\\']).next().unwrap_or(rest);
        if base_name.eq_ignore_ascii_case(filename)
            && hash.len() == 64
            && hash.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

/// Queries the public GitHub Releases API for `carrub-py/ferrum`.
pub fn check_for_updates() -> Result<UpdateCheckResult, String> {
    let agent = http_agent();
    let resp = match agent
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => return Ok(UpdateCheckResult::NoReleasesYet),
        Err(e) => return Err(format!("No se pudo consultar GitHub Releases: {e}")),
    };

    let body_text = resp
        .into_body()
        .read_to_string()
        .map_err(|e| format!("Error leyendo respuesta de GitHub Releases: {e}"))?;
    let release: GithubRelease = serde_json::from_str(&body_text)
        .map_err(|e| format!("Respuesta inválida de GitHub Releases: {e}"))?;

    if release.draft || release.prerelease {
        return Ok(UpdateCheckResult::UpToDate);
    }

    if !is_newer_version(CURRENT_VERSION, &release.tag_name) {
        return Ok(UpdateCheckResult::UpToDate);
    }

    let exe_asset = release
        .assets
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(TARGET_ASSET_NAME))
        .ok_or_else(|| {
            format!(
                "La versión {} aún no contiene el ejecutable {TARGET_ASSET_NAME}.",
                release.tag_name
            )
        })?;

    let sha_asset = release
        .assets
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(CHECKSUMS_ASSET_NAME))
        .ok_or_else(|| {
            format!(
                "La versión {} no incluye {CHECKSUMS_ASSET_NAME} para verificación SHA-256.",
                release.tag_name
            )
        })?;

    if !is_trusted_github_asset_url(&exe_asset.browser_download_url)
        || !is_trusted_github_asset_url(&sha_asset.browser_download_url)
    {
        return Err("URL de descarga fuera del dominio confiable de GitHub.".into());
    }

    let clean_ver = release
        .tag_name
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    let notes = release
        .body
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join("\n");

    Ok(UpdateCheckResult::Available(UpdateInfo {
        version: clean_ver,
        tag: release.tag_name,
        html_url: release.html_url,
        notes,
        exe_download_url: exe_asset.browser_download_url.clone(),
        sha256_url: sha_asset.browser_download_url.clone(),
        exe_size: exe_asset.size,
    }))
}

/// Downloads `SHA256SUMS.txt` and `ferrum-browser.exe`, strictly validates SHA-256,
/// atomically swaps the running Windows executable, and spawns the new version.
pub fn download_verify_and_install(info: &UpdateInfo) -> Result<String, String> {
    if !is_trusted_github_asset_url(&info.exe_download_url)
        || !is_trusted_github_asset_url(&info.sha256_url)
    {
        return Err("Origen de actualización no confiable.".into());
    }

    let agent = download_agent();

    // 1. Fetch and parse mandatory SHA-256 checksum
    let sha_text = agent
        .get(&info.sha256_url)
        .call()
        .map_err(|e| format!("No se pudo descargar {CHECKSUMS_ASSET_NAME}: {e}"))?
        .into_body()
        .read_to_string()
        .map_err(|e| format!("Error leyendo {CHECKSUMS_ASSET_NAME}: {e}"))?;

    let expected_hash = parse_sha256sums(&sha_text, TARGET_ASSET_NAME).ok_or_else(|| {
        format!(
            "No se encontró un hash SHA-256 válido para {TARGET_ASSET_NAME} en {CHECKSUMS_ASSET_NAME}."
        )
    })?;

    // 2. Stream executable to staging file while computing SHA-256
    let current_exe = std::env::current_exe()
        .map_err(|e| format!("No se pudo ubicar el ejecutable actual: {e}"))?;
    let staging_path = PathBuf::from(format!("{}.update", current_exe.to_string_lossy()));
    let backup_path = PathBuf::from(format!("{}.old", current_exe.to_string_lossy()));

    let mut reader = agent
        .get(&info.exe_download_url)
        .call()
        .map_err(|e| format!("Error descargando nueva versión: {e}"))?
        .into_body()
        .into_reader();

    let mut out_file = File::create(&staging_path).map_err(|e| {
        format!("No se pudo escribir en la carpeta del navegador (verifica permisos): {e}")
    })?;

    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total_bytes: u64 = 0;

    loop {
        let n = reader.read(&mut buf).map_err(|e| {
            let _ = fs::remove_file(&staging_path);
            format!("Error de red durante la descarga: {e}")
        })?;
        if n == 0 {
            break;
        }
        total_bytes += n as u64;
        if total_bytes > MAX_UPDATE_BYTES {
            drop(out_file);
            let _ = fs::remove_file(&staging_path);
            return Err("La descarga superó el tamaño máximo permitido (150 MB).".into());
        }
        hasher.update(&buf[..n]);
        out_file.write_all(&buf[..n]).map_err(|e| {
            let _ = fs::remove_file(&staging_path);
            format!("Error escribiendo la actualización en disco: {e}")
        })?;
    }
    out_file.flush().ok();
    drop(out_file);

    // 3. Cryptographic SHA-256 verification before touching the active executable
    let computed_hash = format!("{:x}", hasher.finalize());
    if computed_hash != expected_hash {
        let _ = fs::remove_file(&staging_path);
        return Err(
            "Verificación SHA-256 fallida: el archivo descargado fue alterado o está corrupto. Actualización cancelada."
                .into(),
        );
    }

    // 4. Atomic Windows self-replacement (rename running .exe -> .old, move .update -> .exe)
    let _ = fs::remove_file(&backup_path);
    fs::rename(&current_exe, &backup_path).map_err(|e| {
        let _ = fs::remove_file(&staging_path);
        format!("No se pudo reemplazar el ejecutable actual: {e}")
    })?;

    if let Err(e) = fs::rename(&staging_path, &current_exe) {
        let _ = fs::rename(&backup_path, &current_exe);
        let _ = fs::remove_file(&staging_path);
        return Err(format!("No se pudo activar la nueva versión: {e}"));
    }

    // 5. Launch updated binary (waits briefly so this process persists session & exits cleanly)
    let _ = Command::new(&current_exe).arg("--post-update-wait").spawn();

    Ok(info.version.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_semver_versions_accurately() {
        assert!(is_newer_version("0.5.0", "v0.5.1"));
        assert!(is_newer_version("0.5.0", "0.6.0"));
        assert!(is_newer_version("0.5.0", "v1.0.0"));
        assert!(!is_newer_version("0.5.0", "v0.5.0"));
        assert!(!is_newer_version("0.5.0", "v0.4.9"));
        assert!(!is_newer_version("1.2.3", "v1.2.2"));
    }

    #[test]
    fn parses_sha256sums_and_rejects_malformed_hashes() {
        let valid_hash = "a".repeat(64);
        let other_hash = "b".repeat(64);
        let sums = format!(
            "# Ferrum release hashes\n{other_hash}  ferrum-browser-windows-x64.zip\n{} *ferrum-browser.exe\n",
            valid_hash.to_uppercase()
        );
        assert_eq!(
            parse_sha256sums(&sums, "ferrum-browser.exe").as_deref(),
            Some(valid_hash.as_str())
        );
        assert!(parse_sha256sums("deadbeef  ferrum-browser.exe\n", "ferrum-browser.exe").is_none());
        assert!(parse_sha256sums(&sums, "missing.exe").is_none());
    }

    #[test]
    fn validates_trusted_github_asset_urls() {
        assert!(is_trusted_github_asset_url(
            "https://github.com/carrub-py/ferrum/releases/download/v0.5.1/ferrum-browser.exe"
        ));
        assert!(is_trusted_github_asset_url(
            "https://objects.githubusercontent.com/github-production-release-asset/123"
        ));
        assert!(!is_trusted_github_asset_url(
            "http://github.com/carrub-py/ferrum/releases/download/v0.5.1/ferrum-browser.exe"
        ));
        assert!(!is_trusted_github_asset_url(
            "https://evil.example.com/ferrum-browser.exe"
        ));
    }
}
