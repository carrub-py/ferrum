use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, Key, KeyInit},
};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::storage::unix_now;

#[derive(Debug, Serialize, Deserialize)]
struct BackupBundle {
    version: u8,
    created_at: u64,
    bookmarks_json: Option<String>,
    settings_json: Option<String>,
    notes_json: Option<String>,
    userscripts_json: Option<String>,
    vault_enc: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct EncryptedBackupFile {
    magic: String,
    version: u8,
    salt_b64: String,
    nonce_b64: String,
    ciphertext_b64: String,
}

/// Exports all Ferrum user data (bookmarks, settings, notes, userscripts, encrypted vault)
/// into a single Argon2id + AES-256-GCM encrypted `.ferrum-backup` file.
pub fn export_encrypted_backup(
    data_dir: &Path,
    target_path: &Path,
    passphrase: &str,
) -> Result<PathBuf, String> {
    if passphrase.chars().count() < 6 {
        return Err("Usa una clave de al menos 6 caracteres para cifrar el respaldo.".into());
    }

    let bundle = BackupBundle {
        version: 1,
        created_at: unix_now(),
        bookmarks_json: fs::read_to_string(data_dir.join("bookmarks.json")).ok(),
        settings_json: fs::read_to_string(data_dir.join("settings.json")).ok(),
        notes_json: fs::read_to_string(data_dir.join("notes.json")).ok(),
        userscripts_json: fs::read_to_string(data_dir.join("userscripts.json")).ok(),
        vault_enc: fs::read_to_string(data_dir.join("vault.enc")).ok(),
    };

    let plaintext = serde_json::to_vec(&bundle).map_err(|e| format!("Error al empaquetar: {e}"))?;

    let mut salt = [0u8; 16];
    let mut nonce_arr = [0u8; 12];
    getrandom::fill(&mut salt).map_err(|e| format!("Error de entropía: {e}"))?;
    getrandom::fill(&mut nonce_arr).map_err(|e| format!("Error de entropía: {e}"))?;

    let key_bytes = derive_backup_key(passphrase, &salt)?;
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(key_bytes));
    let ciphertext = cipher
        .encrypt(&Nonce::from(nonce_arr), plaintext.as_ref())
        .map_err(|_| "Error cifrando el respaldo.".to_string())?;

    let envelope = EncryptedBackupFile {
        magic: "FERRUM_BACKUP_V1".to_string(),
        version: 1,
        salt_b64: B64.encode(salt),
        nonce_b64: B64.encode(nonce_arr),
        ciphertext_b64: B64.encode(ciphertext),
    };

    if let Some(parent) = target_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(&envelope)
        .map_err(|e| format!("Error serializando respaldo: {e}"))?;
    fs::write(target_path, json).map_err(|e| format!("No se pudo escribir el archivo: {e}"))?;
    Ok(target_path.to_path_buf())
}

/// Decrypts and restores a `.ferrum-backup` file into `data_dir`.
pub fn import_encrypted_backup(
    data_dir: &Path,
    source_path: &Path,
    passphrase: &str,
) -> Result<usize, String> {
    if !source_path.exists() {
        return Err(format!(
            "No se encontró el archivo de respaldo: {}",
            source_path.display()
        ));
    }

    let raw =
        fs::read_to_string(source_path).map_err(|e| format!("No se pudo leer el respaldo: {e}"))?;
    let env: EncryptedBackupFile =
        serde_json::from_str(&raw).map_err(|_| "Archivo de respaldo inválido.".to_string())?;
    if env.magic != "FERRUM_BACKUP_V1" {
        return Err("Formato de respaldo no reconocido.".into());
    }

    let salt: [u8; 16] = B64
        .decode(&env.salt_b64)
        .map_err(|_| "Salt inválido.".to_string())?
        .try_into()
        .map_err(|_| "Longitud de salt incorrecta.".to_string())?;
    let nonce_arr: [u8; 12] = B64
        .decode(&env.nonce_b64)
        .map_err(|_| "Nonce inválido.".to_string())?
        .try_into()
        .map_err(|_| "Longitud de nonce incorrecta.".to_string())?;
    let ciphertext = B64
        .decode(&env.ciphertext_b64)
        .map_err(|_| "Datos cifrados corruptos.".to_string())?;

    let key_bytes = derive_backup_key(passphrase, &salt)?;
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(key_bytes));
    let plaintext = cipher
        .decrypt(&Nonce::from(nonce_arr), ciphertext.as_ref())
        .map_err(|_| "Clave de respaldo incorrecta.".to_string())?;

    let bundle: BackupBundle = serde_json::from_slice(&plaintext)
        .map_err(|_| "Contenido de respaldo dañado.".to_string())?;

    let _ = fs::create_dir_all(data_dir);
    let mut restored = 0usize;
    let items = [
        ("bookmarks.json", bundle.bookmarks_json),
        ("settings.json", bundle.settings_json),
        ("notes.json", bundle.notes_json),
        ("userscripts.json", bundle.userscripts_json),
        ("vault.enc", bundle.vault_enc),
    ];
    for (filename, content) in items {
        if let Some(body) = content {
            fs::write(data_dir.join(filename), body)
                .map_err(|e| format!("Error restaurando {filename}: {e}"))?;
            restored += 1;
        }
    }
    Ok(restored)
}

fn derive_backup_key(passphrase: &str, salt: &[u8; 16]) -> Result<[u8; 32], String> {
    let params = Params::new(32 * 1024, 2, 1, Some(32))
        .map_err(|e| format!("Parámetros Argon2 inválidos: {e}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| format!("Error derivando clave: {e}"))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_encrypted_backup_bundle() {
        let base = std::env::temp_dir().join(format!("ferrum_bkp_test_{}", unix_now()));
        let src = base.join("src");
        let dst = base.join("dst");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("bookmarks.json"), r#"[{"id":"1"}]"#).unwrap();
        fs::write(src.join("notes.json"), r#"[{"id":"n1"}]"#).unwrap();

        let file = base.join("test.ferrum-backup");
        export_encrypted_backup(&src, &file, "ClaveRespaldo!2026").unwrap();

        assert!(import_encrypted_backup(&dst, &file, "otra_clave").is_err());
        let count = import_encrypted_backup(&dst, &file, "ClaveRespaldo!2026").unwrap();
        assert_eq!(count, 2);
        assert!(dst.join("bookmarks.json").exists());
        assert!(dst.join("notes.json").exists());

        let _ = fs::remove_dir_all(&base);
    }
}
