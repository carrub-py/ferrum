use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::storage::{generate_id, unix_now};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserScript {
    pub id: String,
    pub name: String,
    /// Comma-separated host patterns (e.g. `*`, `youtube.com`, `*.github.com`).
    pub match_pattern: String,
    pub code: String,
    pub enabled: bool,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChromiumExtensionInfo {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

pub struct UserScriptStore {
    file_path: PathBuf,
    pub scripts: Vec<UserScript>,
}

impl UserScriptStore {
    pub fn load(data_dir: PathBuf) -> Self {
        let file_path = data_dir.join("userscripts.json");
        let scripts = if file_path.exists() {
            fs::read_to_string(&file_path)
                .ok()
                .and_then(|s| serde_json::from_str::<Vec<UserScript>>(&s).ok())
                .unwrap_or_else(starter_userscripts)
        } else {
            let defaults = starter_userscripts();
            let _ = write_json(&file_path, &defaults);
            defaults
        };
        Self { file_path, scripts }
    }

    pub fn reload(&mut self) {
        if let Ok(raw) = fs::read_to_string(&self.file_path)
            && let Ok(parsed) = serde_json::from_str::<Vec<UserScript>>(&raw)
        {
            self.scripts = parsed;
        }
    }

    pub fn add(&mut self, name: String, match_pattern: String, code: String) -> bool {
        let clean_name = name.trim().to_string();
        let clean_code = code.trim().to_string();
        if clean_name.is_empty() || clean_code.is_empty() {
            return false;
        }
        let pat = if match_pattern.trim().is_empty() {
            "*".to_string()
        } else {
            match_pattern.trim().to_string()
        };
        self.scripts.insert(
            0,
            UserScript {
                id: generate_id("us"),
                name: clean_name,
                match_pattern: pat,
                code: clean_code,
                enabled: true,
                updated_at: unix_now(),
            },
        );
        self.save();
        true
    }

    pub fn toggle(&mut self, id: &str) {
        if let Some(s) = self.scripts.iter_mut().find(|s| s.id == id) {
            s.enabled = !s.enabled;
            self.save();
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.scripts.retain(|s| s.id != id);
        self.save();
    }

    pub fn matching_scripts_for_host(&self, host: &str) -> Vec<&UserScript> {
        let h = host.to_ascii_lowercase();
        self.scripts
            .iter()
            .filter(|s| s.enabled && pattern_matches_host(&s.match_pattern, &h))
            .collect()
    }

    fn save(&self) {
        let _ = write_json(&self.file_path, &self.scripts);
    }
}

pub fn pattern_matches_host(pattern_list: &str, host: &str) -> bool {
    let clean_host = host.trim().to_ascii_lowercase();
    if clean_host.is_empty() {
        return false;
    }
    for raw in pattern_list.split(',') {
        let p = raw.trim().to_ascii_lowercase();
        if p.is_empty() {
            continue;
        }
        if p == "*" || p == "*://*/*" {
            return true;
        }
        let stripped = p
            .strip_prefix("https://")
            .or_else(|| p.strip_prefix("http://"))
            .unwrap_or(&p)
            .split('/')
            .next()
            .unwrap_or(&p);
        if let Some(suffix) = stripped.strip_prefix("*.") {
            if clean_host == suffix || clean_host.ends_with(&format!(".{suffix}")) {
                return true;
            }
        } else if clean_host == stripped || clean_host.ends_with(&format!(".{stripped}")) {
            return true;
        }
    }
    false
}

fn starter_userscripts() -> Vec<UserScript> {
    let now = unix_now();
    vec![
        UserScript {
            id: "us_unlock_copy".into(),
            name: "Permitir selección y copia bloqueada".into(),
            match_pattern: "*".into(),
            code: r#"(function(){
  const s = document.createElement('style');
  s.textContent = '* { user-select: text !important; -webkit-user-select: text !important; }';
  (document.head || document.documentElement).appendChild(s);
  ['copy','cut','selectstart','contextmenu'].forEach(ev => {
    window.addEventListener(ev, e => e.stopPropagation(), true);
  });
})();"#
                .into(),
            enabled: false,
            updated_at: now,
        },
        UserScript {
            id: "us_yt_speed".into(),
            name: "YouTube: botón rápido velocidad 1.5x".into(),
            match_pattern: "youtube.com".into(),
            code: r#"(function(){
  document.addEventListener('keydown', e => {
    if (e.altKey && e.key.toLowerCase() === 's') {
      const v = document.querySelector('video');
      if (v) v.playbackRate = v.playbackRate === 1.5 ? 1.0 : 1.5;
    }
  });
})();"#
                .into(),
            enabled: false,
            updated_at: now,
        },
    ]
}

fn write_json<T: Serialize>(path: &PathBuf, value: &T) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let content = serde_json::to_string_pretty(value)?;
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path).or_else(|_| fs::write(path, serde_json::to_string_pretty(value)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_userscript_host_patterns() {
        assert!(pattern_matches_host("*", "example.com"));
        assert!(pattern_matches_host("*.github.com", "gist.github.com"));
        assert!(pattern_matches_host("youtube.com", "www.youtube.com"));
        assert!(!pattern_matches_host("github.com", "gitlab.com"));
    }
}
