use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, Key, KeyInit},
};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use zeroize::Zeroize;

use crate::storage::unix_now;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultCredential {
    pub id: String,
    pub site: String,
    pub username: String,
    pub password: String,
    pub notes: String,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedVaultFile {
    version: u8,
    salt_b64: String,
    nonce_b64: String,
    ciphertext_b64: String,
}

pub struct PasswordVault {
    file_path: PathBuf,
    master_key: Option<[u8; 32]>,
    salt: Option<[u8; 16]>,
    entries: Vec<VaultCredential>,
    last_activity: Instant,
}

impl Drop for PasswordVault {
    fn drop(&mut self) {
        self.lock();
    }
}

impl PasswordVault {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            file_path: data_dir.join("vault.enc"),
            master_key: None,
            salt: None,
            entries: Vec::new(),
            last_activity: Instant::now(),
        }
    }

    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    pub fn check_auto_lock(&mut self, minutes: u32) -> bool {
        if minutes == 0 || !self.is_unlocked() {
            return false;
        }
        if self.last_activity.elapsed() >= Duration::from_secs(u64::from(minutes) * 60) {
            self.lock();
            true
        } else {
            false
        }
    }

    pub fn exists(&self) -> bool {
        self.file_path.exists()
    }

    pub fn is_unlocked(&self) -> bool {
        self.master_key.is_some()
    }

    pub fn entries(&self) -> &[VaultCredential] {
        &self.entries
    }

    pub fn lock(&mut self) {
        if let Some(ref mut k) = self.master_key {
            k.zeroize();
        }
        self.master_key = None;
        if let Some(ref mut s) = self.salt {
            s.zeroize();
        }
        self.salt = None;
        for entry in &mut self.entries {
            entry.password.zeroize();
            entry.username.zeroize();
            entry.notes.zeroize();
        }
        self.entries.clear();
    }

    /// Initialize a brand-new encrypted vault with a master password (min 6 chars).
    pub fn setup(&mut self, master_password: &str) -> Result<(), String> {
        if master_password.chars().count() < 6 {
            return Err("La clave maestra debe tener al menos 6 caracteres.".to_string());
        }
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).map_err(|e| format!("Error de entropía: {e}"))?;
        let key = derive_key(master_password, &salt)?;
        self.master_key = Some(key);
        self.salt = Some(salt);
        self.entries.clear();
        self.touch();
        self.save()
    }

    /// Unlock an existing encrypted vault with the master password.
    pub fn unlock(&mut self, master_password: &str) -> Result<(), String> {
        let raw = fs::read_to_string(&self.file_path)
            .map_err(|_| "La bóveda todavía no está configurada.".to_string())?;
        let file: EncryptedVaultFile =
            serde_json::from_str(&raw).map_err(|_| "Formato de bóveda dañado.".to_string())?;

        let salt_vec = B64
            .decode(&file.salt_b64)
            .map_err(|_| "Salt inválido en bóveda.".to_string())?;
        let nonce_vec = B64
            .decode(&file.nonce_b64)
            .map_err(|_| "Nonce inválido en bóveda.".to_string())?;
        let ciphertext = B64
            .decode(&file.ciphertext_b64)
            .map_err(|_| "Datos cifrados inválidos.".to_string())?;

        let salt: [u8; 16] = salt_vec
            .try_into()
            .map_err(|_| "Longitud de salt incorrecta.".to_string())?;
        let nonce_arr: [u8; 12] = nonce_vec
            .try_into()
            .map_err(|_| "Longitud de nonce incorrecta.".to_string())?;

        let key_bytes = derive_key(master_password, &salt)?;
        let key = Key::<Aes256Gcm>::from(key_bytes);
        let cipher = Aes256Gcm::new(&key);
        let nonce = Nonce::from(nonce_arr);

        let mut plaintext = cipher
            .decrypt(&nonce, ciphertext.as_ref())
            .map_err(|_| "Clave maestra incorrecta.".to_string())?;

        let parsed = serde_json::from_slice::<Vec<VaultCredential>>(&plaintext);
        plaintext.zeroize();
        let entries =
            parsed.map_err(|_| "No se pudieron leer las credenciales descifradas.".to_string())?;

        self.master_key = Some(key_bytes);
        self.salt = Some(salt);
        self.entries = entries;
        self.touch();
        Ok(())
    }

    pub fn add_credential(
        &mut self,
        site: String,
        username: String,
        password: String,
        notes: String,
    ) -> Result<(), String> {
        if !self.is_unlocked() {
            return Err("Desbloquea la bóveda primero.".to_string());
        }
        let clean_site = site.trim().to_string();
        if clean_site.is_empty() || password.is_empty() {
            return Err("El sitio y la contraseña no pueden estar vacíos.".to_string());
        }
        let now = unix_now();
        let id = format!(
            "cred_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        self.entries.insert(
            0,
            VaultCredential {
                id,
                site: clean_site,
                username: username.trim().to_string(),
                password,
                notes: notes.trim().to_string(),
                updated_at: now,
            },
        );
        self.save()
    }

    pub fn remove_credential(&mut self, id: &str) -> Result<(), String> {
        if !self.is_unlocked() {
            return Err("Desbloquea la bóveda primero.".to_string());
        }
        for entry in &mut self.entries {
            if entry.id == id {
                entry.password.zeroize();
                entry.username.zeroize();
                entry.notes.zeroize();
            }
        }
        self.entries.retain(|c| c.id != id);
        self.save()
    }

    pub fn find_by_id(&self, id: &str) -> Option<&VaultCredential> {
        self.entries.iter().find(|c| c.id == id)
    }

    pub fn find_for_host(&self, host: &str) -> Option<&VaultCredential> {
        let h = normalize_site_host(host);
        if h.is_empty() {
            return None;
        }
        self.entries.iter().find(|c| {
            let cs = normalize_site_host(&c.site);
            !cs.is_empty()
                && (cs == h || h.ends_with(&format!(".{cs}")) || cs.ends_with(&format!(".{h}")))
        })
    }

    pub fn has_credentials_for_host(&self, host: &str) -> bool {
        self.is_unlocked() && self.find_for_host(host).is_some()
    }

    /// Imports passwords from Chrome / Edge / Brave / Bitwarden CSV exports.
    pub fn import_csv(&mut self, csv_text: &str) -> Result<usize, String> {
        if !self.is_unlocked() {
            return Err("Desbloquea la bóveda primero para importar contraseñas.".into());
        }
        let rows = parse_csv(csv_text);
        if rows.len() < 2 {
            return Err("El archivo CSV no contiene registros de contraseñas.".into());
        }

        let header: Vec<String> = rows[0]
            .iter()
            .map(|h| h.trim().trim_start_matches('\u{feff}').to_ascii_lowercase())
            .collect();

        let idx_url = header.iter().position(|h| {
            matches!(
                h.as_str(),
                "url" | "login_uri" | "origin" | "site" | "website"
            )
        });
        let idx_name = header
            .iter()
            .position(|h| matches!(h.as_str(), "name" | "title"));
        let idx_user = header
            .iter()
            .position(|h| matches!(h.as_str(), "username" | "login_username" | "user" | "email"));
        let idx_pass = header
            .iter()
            .position(|h| matches!(h.as_str(), "password" | "login_password" | "pass"));
        let idx_notes = header
            .iter()
            .position(|h| matches!(h.as_str(), "note" | "notes"));

        let Some(p_idx) = idx_pass else {
            return Err(
                "No se encontró columna de contraseña (password / login_password) en el CSV."
                    .into(),
            );
        };

        let mut imported = 0usize;
        let now = unix_now();
        for (row_num, row) in rows.iter().skip(1).enumerate() {
            let password = row.get(p_idx).map(|s| s.trim()).unwrap_or("");
            if password.is_empty() {
                continue;
            }
            let raw_url = idx_url
                .and_then(|i| row.get(i))
                .map(|s| s.trim())
                .unwrap_or("");
            let raw_name = idx_name
                .and_then(|i| row.get(i))
                .map(|s| s.trim())
                .unwrap_or("");
            let site_src = if !raw_url.is_empty() {
                raw_url
            } else {
                raw_name
            };
            let site = normalize_site_host(site_src);
            if site.is_empty() {
                continue;
            }
            let username = idx_user
                .and_then(|i| row.get(i))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let notes = idx_notes
                .and_then(|i| row.get(i))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            let duplicate = self.entries.iter().any(|e| {
                normalize_site_host(&e.site) == site
                    && e.username.eq_ignore_ascii_case(&username)
                    && e.password == password
            });
            if duplicate {
                continue;
            }

            self.entries.insert(
                0,
                VaultCredential {
                    id: format!("cred_{now}_{row_num}"),
                    site,
                    username,
                    password: password.to_string(),
                    notes,
                    updated_at: now,
                },
            );
            imported += 1;
        }

        self.touch();
        self.save()?;
        Ok(imported)
    }

    pub fn export_csv(&self) -> Result<String, String> {
        if !self.is_unlocked() {
            return Err("Desbloquea la bóveda primero.".into());
        }
        let mut out = String::from("name,url,username,password,note\n");
        for e in &self.entries {
            let url = if e.site.contains("://") {
                e.site.clone()
            } else {
                format!("https://{}", e.site)
            };
            out.push_str(&format!(
                "{},{},{},{},{}\n",
                csv_escape(&e.site),
                csv_escape(&url),
                csv_escape(&e.username),
                csv_escape(&e.password),
                csv_escape(&e.notes)
            ));
        }
        Ok(out)
    }

    fn save(&self) -> Result<(), String> {
        let key_bytes = self
            .master_key
            .ok_or_else(|| "Bóveda bloqueada.".to_string())?;
        let salt = self.salt.ok_or_else(|| "Salt faltante.".to_string())?;

        let mut plaintext = serde_json::to_vec(&self.entries)
            .map_err(|e| format!("Error al serializar credenciales: {e}"))?;

        let mut nonce_arr = [0u8; 12];
        getrandom::fill(&mut nonce_arr).map_err(|e| format!("Error de entropía: {e}"))?;

        let key = Key::<Aes256Gcm>::from(key_bytes);
        let cipher = Aes256Gcm::new(&key);
        let nonce = Nonce::from(nonce_arr);

        let enc_res = cipher.encrypt(&nonce, plaintext.as_ref());
        plaintext.zeroize();
        let ciphertext = enc_res.map_err(|_| "Error al cifrar la bóveda.".to_string())?;

        let payload = EncryptedVaultFile {
            version: 1,
            salt_b64: B64.encode(salt),
            nonce_b64: B64.encode(nonce_arr),
            ciphertext_b64: B64.encode(ciphertext),
        };

        let json = serde_json::to_string_pretty(&payload)
            .map_err(|e| format!("Error al codificar archivo: {e}"))?;
        let tmp = self.file_path.with_extension("enc.tmp");
        fs::write(&tmp, &json).map_err(|e| format!("No se pudo guardar la bóveda: {e}"))?;
        fs::rename(&tmp, &self.file_path)
            .or_else(|_| fs::write(&self.file_path, &json))
            .map_err(|e| format!("Error al escribir bóveda: {e}"))
    }
}

/// Cryptographically strong password generator using OS CSPRNG (`getrandom`).
pub fn generate_secure_password(length: usize, include_symbols: bool) -> String {
    const LOWER: &[u8] = b"abcdefghijkmnopqrstuvwxyz";
    const UPPER: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    const DIGITS: &[u8] = b"23456789";
    const SYMBOLS: &[u8] = b"!@#$%&*()-_=+?[]";

    let len = length.clamp(10, 48);
    let mut pool = Vec::with_capacity(90);
    pool.extend_from_slice(LOWER);
    pool.extend_from_slice(UPPER);
    pool.extend_from_slice(DIGITS);
    if include_symbols {
        pool.extend_from_slice(SYMBOLS);
    }

    let mut raw = vec![0u8; len + 8];
    let _ = getrandom::fill(&mut raw);

    let mut chars = Vec::with_capacity(len);
    // Guarantee character class diversity
    chars.push(LOWER[(raw[0] as usize) % LOWER.len()] as char);
    chars.push(UPPER[(raw[1] as usize) % UPPER.len()] as char);
    chars.push(DIGITS[(raw[2] as usize) % DIGITS.len()] as char);
    if include_symbols {
        chars.push(SYMBOLS[(raw[3] as usize) % SYMBOLS.len()] as char);
    }

    let mut i = 4;
    while chars.len() < len {
        let b = raw.get(i).copied().unwrap_or(0) as usize;
        chars.push(pool[b % pool.len()] as char);
        i += 1;
    }

    // Fisher-Yates shuffle with OS entropy
    let mut shuf = vec![0u8; len];
    let _ = getrandom::fill(&mut shuf);
    for idx in (1..chars.len()).rev() {
        let swap_idx = (shuf[idx] as usize) % (idx + 1);
        chars.swap(idx, swap_idx);
    }

    chars.into_iter().collect()
}

fn normalize_site_host(raw: &str) -> String {
    let s = raw.trim().to_ascii_lowercase();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(&s);
    let host = s.split('/').next().unwrap_or(s);
    let host = host.split(':').next().unwrap_or(host);
    host.trim_start_matches("www.")
        .trim_matches('.')
        .to_string()
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn parse_csv(input: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => {
                    row.push(std::mem::take(&mut field));
                }
                '\r' => {}
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    if row.iter().any(|cell| !cell.trim().is_empty()) {
                        rows.push(std::mem::take(&mut row));
                    } else {
                        row.clear();
                    }
                }
                _ => field.push(c),
            }
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        if row.iter().any(|cell| !cell.trim().is_empty()) {
            rows.push(row);
        }
    }
    rows
}

fn derive_key(password: &str, salt: &[u8; 16]) -> Result<[u8; 32], String> {
    // Balanced Argon2id parameters (32 MiB memory, 2 iterations) — strong & responsive on desktop
    let params = Params::new(32 * 1024, 2, 1, Some(32))
        .map_err(|e| format!("Parámetros Argon2 inválidos: {e}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| format!("Error derivando clave Argon2id: {e}"))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypts_unlocks_and_rejects_wrong_password() {
        let dir = std::env::temp_dir().join(format!("ferrum_vault_test_{}", unix_now()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        {
            let mut v = PasswordVault::new(dir.clone());
            v.setup("MiClaveSegura!2026").unwrap();
            v.add_credential(
                "github.com".into(),
                "ferrum@dev.org".into(),
                "SuperSecret#99".into(),
                "Cuenta principal".into(),
            )
            .unwrap();
            v.lock();
            assert!(!v.is_unlocked());
            assert!(v.entries().is_empty());
        }

        {
            let mut v = PasswordVault::new(dir.clone());
            assert!(v.exists());
            assert!(v.unlock("clave_equivocada").is_err());
            v.unlock("MiClaveSegura!2026").unwrap();
            assert_eq!(v.entries().len(), 1);
            assert_eq!(v.entries()[0].username, "ferrum@dev.org");
            assert_eq!(v.entries()[0].password, "SuperSecret#99");
            assert!(v.has_credentials_for_host("www.github.com"));

            let csv = "name,url,username,password,note\nExample,https://example.com/login,ana@example.com,\"Pass,123\",nota\n";
            assert_eq!(v.import_csv(csv).unwrap(), 1);
            assert!(v.has_credentials_for_host("example.com"));
            let exported = v.export_csv().unwrap();
            assert!(exported.contains("ana@example.com"));
        }

        let pwd = generate_secure_password(20, true);
        assert_eq!(pwd.len(), 20);

        let _ = fs::remove_dir_all(&dir);
    }
}
