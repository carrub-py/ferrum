use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::navigation::{SearchEngine, is_internal_url};

const MAX_HISTORY_ITEMS: usize = 3000;
const MAX_DOWNLOAD_ITEMS: usize = 120;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutMode {
    #[default]
    Minimal,
    Compact,
    Vertical,
}

impl LayoutMode {
    pub fn from_id(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "compact" => Self::Compact,
            "vertical" => Self::Vertical,
            _ => Self::Minimal,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: String,
    pub title: String,
    pub url: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub title: String,
    pub url: String,
    pub visited_at: u64,
    pub visit_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    InProgress,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadItem {
    pub id: String,
    pub filename: String,
    pub url: String,
    pub path: String,
    pub status: DownloadStatus,
    pub started_at: u64,
    pub bytes_received: u64,
    pub total_bytes: Option<u64>,
    #[serde(default)]
    pub speed_bps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteItem {
    pub id: String,
    pub title: String,
    pub content: String,
    pub source_url: Option<String>,
    pub source_title: Option<String>,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub accent: String,
    #[serde(default)]
    pub isolated_container: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedTab {
    pub url: String,
    pub title: String,
    pub pinned: bool,
    pub workspace_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionData {
    pub tabs: Vec<SavedTab>,
    pub active_index: usize,
    pub active_workspace_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    Deny,
    Ask,
    Allow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionSettings {
    pub notifications: PermissionMode,
    pub geolocation: PermissionMode,
    pub camera_mic: PermissionMode,
    pub sensors: PermissionMode,
    pub clipboard: PermissionMode,
}

impl Default for PermissionSettings {
    fn default() -> Self {
        Self {
            notifications: PermissionMode::Deny,
            geolocation: PermissionMode::Deny,
            camera_mic: PermissionMode::Ask,
            sensors: PermissionMode::Deny,
            clipboard: PermissionMode::Ask,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserSettings {
    pub search_engine: SearchEngine,
    pub layout_mode: LayoutMode,
    pub vertical_collapsed: bool,
    pub show_bookmarks_bar: bool,
    pub restore_session: bool,
    pub auto_hibernate_minutes: u32,
    pub live_search_suggestions: bool,
    pub vault_auto_lock_minutes: u32,
    pub sync_folder_path: Option<String>,
    pub domain_zoom_levels: HashMap<String, f64>,
    pub custom_cosmetic_rules: HashMap<String, Vec<String>>,
    pub https_only: bool,
    pub shield_enabled: bool,
    pub cosmetic_filtering: bool,
    pub block_cookie_banners: bool,
    pub anti_fingerprint: bool,
    pub strip_tracking_params: bool,
    pub filter_lists_updated_at: u64,
    pub filter_lists_rules_count: usize,
    pub permissions: PermissionSettings,
    pub workspaces: Vec<Workspace>,
    pub allowlisted_domains: Vec<String>,
    pub custom_blocked_domains: Vec<String>,
    pub total_blocked_lifetime: u64,
    pub bytes_saved_lifetime: u64,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            search_engine: SearchEngine::DuckDuckGo,
            layout_mode: LayoutMode::Minimal,
            vertical_collapsed: false,
            show_bookmarks_bar: false,
            restore_session: true,
            auto_hibernate_minutes: 15,
            live_search_suggestions: true,
            vault_auto_lock_minutes: 15,
            sync_folder_path: None,
            domain_zoom_levels: HashMap::new(),
            custom_cosmetic_rules: HashMap::new(),
            https_only: true,
            shield_enabled: true,
            cosmetic_filtering: true,
            block_cookie_banners: true,
            anti_fingerprint: true,
            strip_tracking_params: true,
            filter_lists_updated_at: 0,
            filter_lists_rules_count: 0,
            permissions: PermissionSettings::default(),
            workspaces: default_workspaces(),
            allowlisted_domains: Vec::new(),
            custom_blocked_domains: Vec::new(),
            total_blocked_lifetime: 0,
            bytes_saved_lifetime: 0,
        }
    }
}

pub struct BrowserStorage {
    data_dir: PathBuf,
    pub bookmarks: Vec<Bookmark>,
    pub history: Vec<HistoryEntry>,
    pub downloads: Vec<DownloadItem>,
    pub notes: Vec<NoteItem>,
    pub settings: BrowserSettings,
}

impl BrowserStorage {
    pub fn load() -> Self {
        let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
        let data_dir = base.join("FerrumBrowser");
        Self::load_from_dir(data_dir)
    }

    pub fn load_from_dir(data_dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&data_dir);

        let bookmarks_path = data_dir.join("bookmarks.json");
        let bookmarks: Vec<Bookmark> = if bookmarks_path.exists() {
            fs::read_to_string(&bookmarks_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(default_bookmarks)
        } else {
            let defaults = default_bookmarks();
            let _ = write_json(&bookmarks_path, &defaults);
            defaults
        };

        let history: Vec<HistoryEntry> = fs::read_to_string(data_dir.join("history.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let mut downloads: Vec<DownloadItem> = fs::read_to_string(data_dir.join("downloads.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // Clean stale in-progress items from previous runs
        for d in &mut downloads {
            if matches!(
                d.status,
                DownloadStatus::InProgress | DownloadStatus::Paused
            ) {
                d.status = DownloadStatus::Cancelled;
            }
        }

        let notes: Vec<NoteItem> = fs::read_to_string(data_dir.join("notes.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let mut settings: BrowserSettings = fs::read_to_string(data_dir.join("settings.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        if settings.workspaces.is_empty() {
            settings.workspaces = default_workspaces();
        }

        Self {
            data_dir,
            bookmarks,
            history,
            downloads,
            notes,
            settings,
        }
    }

    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    pub fn reload_from_disk(&mut self) {
        let fresh = Self::load_from_dir(self.data_dir.clone());
        self.bookmarks = fresh.bookmarks;
        self.notes = fresh.notes;
        self.settings = fresh.settings;
    }

    pub fn extensions_dir(&self) -> PathBuf {
        let dir = self.data_dir.join("extensions");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    pub fn domain_zoom(&self, host: &str) -> f64 {
        let key = host.trim().to_ascii_lowercase();
        self.settings
            .domain_zoom_levels
            .get(&key)
            .copied()
            .unwrap_or(1.0)
            .clamp(0.5, 3.0)
    }

    pub fn set_domain_zoom(&mut self, host: &str, factor: f64) {
        let key = host.trim().to_ascii_lowercase();
        if key.is_empty() {
            return;
        }
        let clamped = (factor.clamp(0.5, 3.0) * 100.0).round() / 100.0;
        if (clamped - 1.0).abs() < 0.01 {
            self.settings.domain_zoom_levels.remove(&key);
        } else {
            self.settings.domain_zoom_levels.insert(key, clamped);
        }
        self.save_settings();
    }

    pub fn add_custom_cosmetic_rule(&mut self, host: &str, selector: &str) -> bool {
        let h = host.trim().to_ascii_lowercase();
        let s = selector.trim().to_string();
        if h.is_empty() || s.is_empty() {
            return false;
        }
        let list = self.settings.custom_cosmetic_rules.entry(h).or_default();
        if list.contains(&s) {
            return false;
        }
        list.push(s);
        self.save_settings();
        true
    }

    pub fn remove_custom_cosmetic_rule(&mut self, host: &str, selector: &str) {
        let h = host.trim().to_ascii_lowercase();
        if let Some(list) = self.settings.custom_cosmetic_rules.get_mut(&h) {
            list.retain(|item| item != selector);
            if list.is_empty() {
                self.settings.custom_cosmetic_rules.remove(&h);
            }
            self.save_settings();
        }
    }

    pub fn custom_cosmetic_selectors_for_host(&self, host: &str) -> Vec<String> {
        let h = host.trim().to_ascii_lowercase();
        self.settings
            .custom_cosmetic_rules
            .get(&h)
            .cloned()
            .unwrap_or_default()
    }

    pub fn custom_cosmetic_css_for_host(&self, host: &str) -> Option<String> {
        let list = self.custom_cosmetic_selectors_for_host(host);
        if list.is_empty() {
            None
        } else {
            Some(format!(
                "{} {{ display: none !important; visibility: hidden !important; }}",
                list.join(", ")
            ))
        }
    }

    pub fn filters_dir(&self) -> PathBuf {
        let dir = self.data_dir.join("filters");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    pub fn webview_profile_dir(&self) -> PathBuf {
        let dir = self.data_dir.join("profile");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    pub fn screenshots_dir() -> PathBuf {
        let dir = dirs::picture_dir()
            .or_else(dirs::download_dir)
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Ferrum Capturas");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    pub fn downloads_dir() -> PathBuf {
        dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn unique_download_path(suggested_filename: &str) -> PathBuf {
        let dir = Self::downloads_dir();
        let _ = fs::create_dir_all(&dir);

        let clean_name = std::path::Path::new(suggested_filename)
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or("descarga.bin");

        let candidate = dir.join(clean_name);
        if !candidate.exists() {
            return candidate;
        }

        let stem = candidate
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("descarga");
        let ext = candidate
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();

        for i in 1..=999 {
            let next = dir.join(format!("{stem} ({i}){ext}"));
            if !next.exists() {
                return next;
            }
        }
        dir.join(format!("{stem}_{}{ext}", unix_now()))
    }

    pub fn load_session(&self) -> Option<SessionData> {
        if !self.settings.restore_session {
            return None;
        }
        fs::read_to_string(self.data_dir.join("session.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<SessionData>(&s).ok())
            .filter(|s| !s.tabs.is_empty())
    }

    pub fn save_session(&self, session: &SessionData) {
        let _ = write_json(&self.data_dir.join("session.json"), session);
    }

    pub fn save_bookmarks(&self) {
        let _ = write_json(&self.data_dir.join("bookmarks.json"), &self.bookmarks);
    }

    pub fn save_history(&self) {
        let _ = write_json(&self.data_dir.join("history.json"), &self.history);
    }

    pub fn save_downloads(&self) {
        let _ = write_json(&self.data_dir.join("downloads.json"), &self.downloads);
    }

    pub fn save_notes(&self) {
        let _ = write_json(&self.data_dir.join("notes.json"), &self.notes);
    }

    pub fn save_settings(&self) {
        let _ = write_json(&self.data_dir.join("settings.json"), &self.settings);
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        let norm = normalize_url_key(url);
        self.bookmarks
            .iter()
            .any(|b| normalize_url_key(&b.url) == norm)
    }

    pub fn toggle_bookmark(&mut self, title: &str, url: &str) -> bool {
        if url.is_empty() || is_internal_url(url) {
            return false;
        }
        let norm = normalize_url_key(url);
        if let Some(pos) = self
            .bookmarks
            .iter()
            .position(|b| normalize_url_key(&b.url) == norm)
        {
            self.bookmarks.remove(pos);
            self.save_bookmarks();
            false
        } else {
            let clean_title = if title.trim().is_empty() {
                url.to_string()
            } else {
                title.trim().to_string()
            };
            self.bookmarks.push(Bookmark {
                id: generate_id("bm"),
                title: clean_title,
                url: url.to_string(),
                created_at: unix_now(),
            });
            self.save_bookmarks();
            true
        }
    }

    pub fn add_bookmark(&mut self, title: &str, url: &str) -> bool {
        if url.trim().is_empty() || is_internal_url(url) {
            return false;
        }
        let norm = normalize_url_key(url);
        if self
            .bookmarks
            .iter()
            .any(|b| normalize_url_key(&b.url) == norm)
        {
            return false;
        }
        let clean_title = if title.trim().is_empty() {
            url.trim().to_string()
        } else {
            title.trim().to_string()
        };
        self.bookmarks.push(Bookmark {
            id: generate_id("bm"),
            title: clean_title,
            url: url.trim().to_string(),
            created_at: unix_now(),
        });
        self.save_bookmarks();
        true
    }

    pub fn remove_bookmark(&mut self, id: &str) {
        self.bookmarks.retain(|b| b.id != id);
        self.save_bookmarks();
    }

    pub fn record_visit(&mut self, url: &str, title: &str) {
        if url.is_empty() || is_internal_url(url) {
            return;
        }
        let now = unix_now();
        let clean_title = if title.trim().is_empty() {
            url.to_string()
        } else {
            title.trim().to_string()
        };

        if let Some(first) = self.history.first_mut()
            && first.url == url
            && now.saturating_sub(first.visited_at) < 15
        {
            if !title.trim().is_empty() {
                first.title = clean_title;
            }
            first.visited_at = now;
            self.save_history();
            return;
        }

        let prev_count = if let Some(pos) = self.history.iter().position(|h| h.url == url) {
            let old = self.history.remove(pos);
            old.visit_count
        } else {
            0
        };

        self.history.insert(
            0,
            HistoryEntry {
                id: generate_id("hist"),
                title: clean_title,
                url: url.to_string(),
                visited_at: now,
                visit_count: prev_count.saturating_add(1),
            },
        );

        if self.history.len() > MAX_HISTORY_ITEMS {
            self.history.truncate(MAX_HISTORY_ITEMS);
        }
        self.save_history();
    }

    pub fn update_latest_history_title(&mut self, url: &str, title: &str) {
        if title.trim().is_empty() || is_internal_url(url) {
            return;
        }
        if let Some(entry) = self.history.iter_mut().find(|h| h.url == url)
            && entry.title != title.trim()
        {
            entry.title = title.trim().to_string();
            self.save_history();
        }
    }

    pub fn remove_history_entry(&mut self, id: &str) {
        self.history.retain(|h| h.id != id);
        self.save_history();
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.save_history();
    }

    pub fn record_download_started(
        &mut self,
        id: String,
        url: String,
        path: PathBuf,
        total_bytes: Option<u64>,
    ) {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("archivo")
            .to_string();
        let path_str = path.to_string_lossy().into_owned();

        self.downloads.insert(
            0,
            DownloadItem {
                id,
                filename,
                url,
                path: path_str,
                status: DownloadStatus::InProgress,
                started_at: unix_now(),
                bytes_received: 0,
                total_bytes,
                speed_bps: 0,
            },
        );
        if self.downloads.len() > MAX_DOWNLOAD_ITEMS {
            self.downloads.truncate(MAX_DOWNLOAD_ITEMS);
        }
        self.save_downloads();
    }

    pub fn update_download_progress(&mut self, id: &str, received: u64, total: Option<u64>) {
        if let Some(item) = self.downloads.iter_mut().find(|d| d.id == id) {
            let elapsed = unix_now().saturating_sub(item.started_at).max(1);
            item.bytes_received = received;
            if let Some(t) = total
                && t > 0
            {
                item.total_bytes = Some(t);
            }
            item.speed_bps = received / elapsed;
        }
    }

    pub fn update_download_state(
        &mut self,
        id: &str,
        status: DownloadStatus,
        path: Option<PathBuf>,
    ) {
        if let Some(item) = self.downloads.iter_mut().find(|d| d.id == id) {
            item.status = status;
            if let Some(p) = path {
                if let Ok(meta) = fs::metadata(&p) {
                    item.bytes_received = meta.len();
                    item.total_bytes = Some(meta.len());
                }
                item.path = p.to_string_lossy().into_owned();
            }
            if !matches!(status, DownloadStatus::InProgress) {
                item.speed_bps = 0;
            }
            self.save_downloads();
        }
    }

    pub fn remove_download(&mut self, id: &str) {
        self.downloads.retain(|d| d.id != id);
        self.save_downloads();
    }

    pub fn clear_downloads(&mut self) {
        self.downloads.retain(|d| {
            matches!(
                d.status,
                DownloadStatus::InProgress | DownloadStatus::Paused
            )
        });
        self.save_downloads();
    }

    pub fn add_note(
        &mut self,
        title: String,
        content: String,
        source_url: Option<String>,
        source_title: Option<String>,
    ) {
        let clean_title = if title.trim().is_empty() {
            "Nota rápida".to_string()
        } else {
            title.trim().to_string()
        };
        if content.trim().is_empty() && source_url.is_none() {
            return;
        }
        self.notes.insert(
            0,
            NoteItem {
                id: generate_id("note"),
                title: clean_title,
                content: content.trim().to_string(),
                source_url,
                source_title,
                updated_at: unix_now(),
            },
        );
        self.save_notes();
    }

    pub fn remove_note(&mut self, id: &str) {
        self.notes.retain(|n| n.id != id);
        self.save_notes();
    }

    pub fn add_workspace(&mut self, name: &str, accent: &str, isolated: bool) -> Workspace {
        let clean = name.trim();
        let ws = Workspace {
            id: generate_id("ws"),
            name: if clean.is_empty() {
                "Espacio".to_string()
            } else {
                clean.to_string()
            },
            accent: if accent.trim().is_empty() {
                "#a1a1aa".to_string()
            } else {
                accent.trim().to_string()
            },
            isolated_container: isolated,
        };
        self.settings.workspaces.push(ws.clone());
        self.save_settings();
        ws
    }

    pub fn toggle_workspace_isolation(&mut self, id: &str) -> bool {
        if id == "ws_general" {
            return false;
        }
        if let Some(ws) = self.settings.workspaces.iter_mut().find(|w| w.id == id) {
            ws.isolated_container = !ws.isolated_container;
            let state = ws.isolated_container;
            self.save_settings();
            return state;
        }
        false
    }

    pub fn remove_workspace(&mut self, id: &str) -> bool {
        if id == "ws_general" || self.settings.workspaces.len() <= 1 {
            return false;
        }
        let before = self.settings.workspaces.len();
        self.settings.workspaces.retain(|w| w.id != id);
        if self.settings.workspaces.len() != before {
            self.save_settings();
            true
        } else {
            false
        }
    }
}

fn default_workspaces() -> Vec<Workspace> {
    vec![
        Workspace {
            id: "ws_general".into(),
            name: "General".into(),
            accent: "#e5e5ea".into(),
            isolated_container: false,
        },
        Workspace {
            id: "ws_work".into(),
            name: "Trabajo".into(),
            accent: "#64d2ff".into(),
            isolated_container: true,
        },
        Workspace {
            id: "ws_study".into(),
            name: "Estudio".into(),
            accent: "#30d158".into(),
            isolated_container: true,
        },
    ]
}

fn default_bookmarks() -> Vec<Bookmark> {
    let now = unix_now();
    vec![
        Bookmark {
            id: "bm_rust".into(),
            title: "Rust Lang".into(),
            url: "https://www.rust-lang.org/".into(),
            created_at: now,
        },
        Bookmark {
            id: "bm_ddg".into(),
            title: "DuckDuckGo".into(),
            url: "https://duckduckgo.com/".into(),
            created_at: now,
        },
        Bookmark {
            id: "bm_github".into(),
            title: "GitHub".into(),
            url: "https://github.com/".into(),
            created_at: now,
        },
        Bookmark {
            id: "bm_wiki".into(),
            title: "Wikipedia".into(),
            url: "https://es.wikipedia.org/".into(),
            created_at: now,
        },
        Bookmark {
            id: "bm_crates".into(),
            title: "crates.io".into(),
            url: "https://crates.io/".into(),
            created_at: now,
        },
        Bookmark {
            id: "bm_youtube".into(),
            title: "YouTube".into(),
            url: "https://www.youtube.com/".into(),
            created_at: now,
        },
    ]
}

fn normalize_url_key(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn generate_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{prefix}_{nanos}")
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
    fn persists_bookmarks_history_notes_and_session() {
        let dir = std::env::temp_dir().join(format!("ferrum_v3_test_{}", unix_now()));
        let _ = fs::remove_dir_all(&dir);

        {
            let mut store = BrowserStorage::load_from_dir(dir.clone());
            assert!(!store.bookmarks.is_empty());
            assert!(store.toggle_bookmark("Example", "https://example.com/"));
            store.record_visit("https://example.com/", "Example Domain");
            store.add_note(
                "Idea".into(),
                "Contenido".into(),
                Some("https://example.com/".into()),
                Some("Example".into()),
            );
            store.save_session(&SessionData {
                tabs: vec![SavedTab {
                    url: "https://example.com/".into(),
                    title: "Example".into(),
                    pinned: true,
                    workspace_id: "ws_work".into(),
                }],
                active_index: 0,
                active_workspace_id: "ws_work".into(),
            });
        }

        {
            let store = BrowserStorage::load_from_dir(dir.clone());
            assert!(store.is_bookmarked("https://example.com"));
            assert_eq!(store.history.len(), 1);
            assert_eq!(store.notes.len(), 1);
            let sess = store.load_session().expect("session restored");
            assert_eq!(sess.tabs.len(), 1);
            assert!(sess.tabs[0].pinned);
            assert!(store.settings.workspaces[1].isolated_container);
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
