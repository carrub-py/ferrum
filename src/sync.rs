use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

use crate::backup::{create_encrypted_backup_json, restore_encrypted_backup_json};
use crate::storage::unix_now;

const GIST_FILENAME: &str = "ferrum-sync.ferrum-backup";
const GIST_API_BASE: &str = "https://api.github.com/gists";

#[derive(Debug, Clone)]
pub struct SyncPushResult {
    pub gist_id: String,
    pub synced_at: u64,
}

#[derive(Debug, Clone)]
pub struct SyncPullResult {
    pub restored_files: usize,
    pub synced_at: u64,
}

#[derive(Debug, Deserialize)]
struct GistResponse {
    id: String,
    #[serde(default)]
    files: std::collections::HashMap<String, GistFile>,
}

#[derive(Debug, Deserialize)]
struct GistFile {
    content: Option<String>,
}

/// Validates a GitHub Gist ID (hex/alphanumeric only — prevents URL path injection).
pub fn is_valid_gist_id(id: &str) -> bool {
    let t = id.trim();
    !t.is_empty() && t.len() <= 64 && t.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Protects a GitHub Personal Access Token at rest using Windows DPAPI (`CryptProtectData`),
/// binding ciphertext strictly to the current Windows user account.
#[cfg(target_os = "windows")]
pub fn protect_token_dpapi(token: &str) -> Result<String, String> {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };
    use windows_core::w;

    let bytes = token.as_bytes();
    if bytes.is_empty() {
        return Ok(String::new());
    }
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &in_blob,
            w!("FerrumSyncToken"),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
        .map_err(|e| format!("Error cifrando token con Windows DPAPI: {e}"))?;
        let slice = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize);
        let encoded = B64.encode(slice);
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            out_blob.pbData as _,
        )));
        Ok(encoded)
    }
}

#[cfg(target_os = "windows")]
pub fn unprotect_token_dpapi(blob_b64: &str) -> Result<String, String> {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };

    if blob_b64.trim().is_empty() {
        return Ok(String::new());
    }
    let mut raw = B64
        .decode(blob_b64.trim())
        .map_err(|_| "Token cifrado inválido.".to_string())?;
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: raw.len() as u32,
        pbData: raw.as_mut_ptr(),
    };
    let mut out_blob = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &in_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
        .map_err(|e| format!("No se pudo descifrar el token DPAPI: {e}"))?;
        let slice = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize);
        let decoded = String::from_utf8(slice.to_vec())
            .map_err(|_| "Token descifrado no es UTF-8 válido.".to_string());
        // Zero out decrypted native buffer before freeing
        std::ptr::write_bytes(out_blob.pbData, 0, out_blob.cbData as usize);
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            out_blob.pbData as _,
        )));
        decoded
    }
}

#[cfg(not(target_os = "windows"))]
pub fn protect_token_dpapi(token: &str) -> Result<String, String> {
    Ok(B64.encode(token.as_bytes()))
}

#[cfg(not(target_os = "windows"))]
pub fn unprotect_token_dpapi(blob_b64: &str) -> Result<String, String> {
    let bytes = B64
        .decode(blob_b64.trim())
        .map_err(|_| "Token inválido.".to_string())?;
    String::from_utf8(bytes).map_err(|_| "Token UTF-8 inválido.".to_string())
}

/// Encrypts local Ferrum user data with Argon2id + AES-256-GCM and uploads ONLY the ciphertext
/// to a private GitHub Gist (Zero-Knowledge E2EE).
pub fn push_encrypted_gist(
    data_dir: &Path,
    token: &str,
    existing_gist_id: Option<&str>,
    passphrase: &str,
) -> Result<SyncPushResult, String> {
    let tok = token.trim();
    if tok.is_empty() {
        return Err("Configura primero tu token personal de GitHub (permiso gist).".into());
    }

    let encrypted_json = create_encrypted_backup_json(data_dir, passphrase)?;
    let payload = serde_json::json!({
        "description": "Ferrum Browser — Respaldo sincronizado cifrado E2EE (Argon2id + AES-256-GCM)",
        "public": false,
        "files": {
            GIST_FILENAME: {
                "content": encrypted_json
            }
        }
    });
    let body_str =
        serde_json::to_string(&payload).map_err(|e| format!("Error serializando Gist: {e}"))?;

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();

    let mut resp = if let Some(gid) = existing_gist_id.filter(|id| is_valid_gist_id(id)) {
        agent
            .patch(&format!("{GIST_API_BASE}/{gid}"))
            .header("User-Agent", "FerrumBrowser-Sync/0.6")
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", &format!("Bearer {tok}"))
            .header("Content-Type", "application/json")
            .send(&body_str)
            .map_err(|e| format!("Error actualizando Gist privado: {e}"))?
    } else {
        agent
            .post(GIST_API_BASE)
            .header("User-Agent", "FerrumBrowser-Sync/0.6")
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", &format!("Bearer {tok}"))
            .header("Content-Type", "application/json")
            .send(&body_str)
            .map_err(|e| format!("Error creando Gist privado en GitHub: {e}"))?
    };

    let raw = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("Error leyendo respuesta de GitHub: {e}"))?;
    let parsed: GistResponse = serde_json::from_str(&raw)
        .map_err(|e| format!("Respuesta inesperada de GitHub Gist: {e}"))?;

    Ok(SyncPushResult {
        gist_id: parsed.id,
        synced_at: unix_now(),
    })
}

/// Downloads the encrypted ciphertext blob from a private GitHub Gist, decrypts locally
/// with `passphrase`, and restores bookmarks/settings/notes/userscripts/vault.
pub fn pull_encrypted_gist(
    data_dir: &Path,
    token: &str,
    gist_id: &str,
    passphrase: &str,
) -> Result<SyncPullResult, String> {
    let tok = token.trim();
    if tok.is_empty() {
        return Err("Configura primero tu token personal de GitHub (permiso gist).".into());
    }
    if !is_valid_gist_id(gist_id) {
        return Err("ID de Gist inválido. Introduce el ID alfanumérico del Gist privado.".into());
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();

    let mut resp = agent
        .get(&format!("{GIST_API_BASE}/{}", gist_id.trim()))
        .header("User-Agent", "FerrumBrowser-Sync/0.6")
        .header("Accept", "application/vnd.github+json")
        .header("Authorization", &format!("Bearer {tok}"))
        .call()
        .map_err(|e| format!("Error descargando Gist de GitHub: {e}"))?;

    let raw = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("Error leyendo Gist: {e}"))?;
    let parsed: GistResponse =
        serde_json::from_str(&raw).map_err(|e| format!("Gist inválido: {e}"))?;

    let content = parsed
        .files
        .get(GIST_FILENAME)
        .and_then(|f| f.content.as_deref())
        .ok_or_else(|| format!("El Gist no contiene el archivo cifrado `{GIST_FILENAME}`."))?;

    let restored_files = restore_encrypted_backup_json(data_dir, content, passphrase)?;
    Ok(SyncPullResult {
        restored_files,
        synced_at: unix_now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_gist_id_and_dpapi_roundtrip() {
        assert!(is_valid_gist_id("a1b2c3d4e5f60718293a4b5c"));
        assert!(!is_valid_gist_id(""));
        assert!(!is_valid_gist_id("../other"));
        assert!(!is_valid_gist_id("abc/def"));

        let secret = "ghp_test_token_1234567890";
        let protected = protect_token_dpapi(secret).unwrap();
        assert_ne!(protected, secret);
        let recovered = unprotect_token_dpapi(&protected).unwrap();
        assert_eq!(recovered, secret);
    }
}
