use serde::{Deserialize, Serialize};

use crate::adblock::BlockedEvent;
use crate::autocomplete::OmniboxSuggestion;
use crate::extensions::{ChromiumExtensionInfo, UserScript};
use crate::importer::DetectedBrowser;
use crate::navigation::SearchEngine;
use crate::os_integration::MemoryStats;
use crate::storage::{
    Bookmark, CustomSearchEngine, DownloadItem, HistoryEntry, LayoutMode, NoteItem, PermissionMode,
    PermissionSettings, TabGroup, Workspace,
};
use crate::updater::UpdateInfo;

pub const CHROME_HTML: &str = include_str!("chrome.html");
pub const PWA_CHROME_HTML: &str = include_str!("pwa_chrome.html");
pub const VERTICAL_HTML: &str = include_str!("vertical.html");
pub const OVERLAY_HTML: &str = include_str!("overlay.html");
pub const DIVIDER_HTML: &str = include_str!("divider.html");
pub const PANEL_HTML: &str = include_str!("panel.html");
pub const NEWTAB_HTML: &str = include_str!("newtab.html");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelTab {
    Commands,
    Shield,
    Tasks,
    Vault,
    Extensions,
    Workspaces,
    Bookmarks,
    History,
    Downloads,
    Notes,
    Settings,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClosedTabInfo {
    pub title: String,
    pub url: String,
    pub closed_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingLoginPrompt {
    pub host: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TabSummary {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub display_url: String,
    pub host: Option<String>,
    pub favicon_url: Option<String>,
    pub workspace_id: String,
    pub group_id: Option<String>,
    pub pinned: bool,
    pub is_private: bool,
    pub is_hibernated: bool,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub is_playing_audio: bool,
    pub is_muted: bool,
    pub zoom_percent: u32,
    pub reader_active: bool,
    pub force_dark_active: bool,
    pub blocked_count: u64,
    pub is_bookmarked: bool,
    pub is_internal: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShieldSnapshot {
    pub enabled: bool,
    pub https_only: bool,
    pub cosmetic_enabled: bool,
    pub block_cookie_banners: bool,
    pub anti_fingerprint: bool,
    pub strip_tracking_params: bool,
    pub has_community_lists: bool,
    pub updating_lists: bool,
    pub lists_updated_at: u64,
    pub current_host: Option<String>,
    pub site_allowlisted: bool,
    pub tab_blocked: u64,
    pub total_blocked: u64,
    pub bytes_saved: u64,
    pub rules_count: usize,
    pub recent_blocked: Vec<BlockedEvent>,
    pub allowlisted_domains: Vec<String>,
    pub custom_blocked_domains: Vec<String>,
    pub custom_cosmetic_rules: Vec<String>,
    pub permissions: PermissionSettings,
}

#[derive(Debug, Clone, Serialize)]
pub struct VaultEntrySummary {
    pub id: String,
    pub site: String,
    pub username: String,
    pub notes: String,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VaultSnapshot {
    pub exists: bool,
    pub unlocked: bool,
    pub error: Option<String>,
    pub generated_password: Option<String>,
    pub has_match_for_active: bool,
    pub pending_login: Option<PendingLoginPrompt>,
    pub auto_lock_minutes: u32,
    pub entries: Vec<VaultEntrySummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FindSnapshot {
    pub open: bool,
    pub query: String,
    pub case_sensitive: bool,
    pub current: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteInfoSnapshot {
    pub host: String,
    pub is_https: bool,
    pub is_private: bool,
    pub shield_enabled: bool,
    pub site_allowlisted: bool,
    pub tab_blocked: u64,
    pub zoom_percent: u32,
    pub reader_active: bool,
    pub force_dark_active: bool,
    pub permissions: PermissionSettings,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverlayState {
    pub mode: String,
    pub suggestions: Vec<OmniboxSuggestion>,
    pub selected_index: usize,
    pub site_info: Option<SiteInfoSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncSnapshot {
    pub has_token: bool,
    pub gist_id: Option<String>,
    pub last_sync: u64,
    pub busy: bool,
    pub status_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateSnapshot {
    pub current_version: String,
    pub repo_url: String,
    pub checking: bool,
    pub installing: bool,
    pub available: Option<UpdateInfo>,
    pub status_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiState {
    pub is_pwa_mode: bool,
    pub pwa_title: Option<String>,
    pub tabs: Vec<TabSummary>,
    pub active_tab_id: u64,
    pub split_tab_id: Option<u64>,
    pub split_ratio: f64,
    pub context_tab_id: Option<u64>,
    pub audio_hub_open: bool,
    pub active_workspace_id: String,
    pub workspaces: Vec<Workspace>,
    pub tab_groups: Vec<TabGroup>,
    pub custom_search_engines: Vec<CustomSearchEngine>,
    pub memory: MemoryStats,
    pub sync: SyncSnapshot,
    pub crash_reports_count: usize,
    pub layout_mode: LayoutMode,
    pub vertical_collapsed: bool,
    pub show_bookmarks_bar: bool,
    pub restore_session: bool,
    pub live_search_suggestions: bool,
    pub auto_hibernate_minutes: u32,
    pub windows_registered: bool,
    pub sync_folder_path: Option<String>,
    pub picked_backup_path: Option<String>,
    pub is_fullscreen: bool,
    pub is_maximized: bool,
    pub open_panel: Option<PanelTab>,
    pub find: FindSnapshot,
    pub search_engine: SearchEngine,
    pub bookmarks: Vec<Bookmark>,
    pub detected_browsers: Vec<DetectedBrowser>,
    pub history: Vec<HistoryEntry>,
    pub closed_tabs: Vec<ClosedTabInfo>,
    pub downloads: Vec<DownloadItem>,
    pub active_downloads_count: usize,
    pub notes: Vec<NoteItem>,
    pub extensions: Vec<ChromiumExtensionInfo>,
    pub userscripts: Vec<UserScript>,
    pub shield: ShieldSnapshot,
    pub vault: VaultSnapshot,
    pub updater: UpdateSnapshot,
    pub toast: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NewTabData {
    pub search_engine: SearchEngine,
    pub total_blocked: u64,
    pub bytes_saved: u64,
    pub rules_count: usize,
    pub shield_enabled: bool,
    pub https_only: bool,
    pub anti_fingerprint: bool,
    pub is_private: bool,
    pub workspace_name: String,
    pub bookmarks: Vec<Bookmark>,
    pub recent_history: Vec<HistoryEntry>,
    pub closed_tabs: Vec<ClosedTabInfo>,
    pub show_onboarding: bool,
    pub layout_mode: LayoutMode,
    pub windows_registered: bool,
    pub detected_browsers: Vec<DetectedBrowser>,
    pub recovered_from_crash: bool,
}

/// Commands sent from trusted internal WebViews (chrome, pwa_chrome, vertical, overlay, divider, panel, newtab) to Rust.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcCommand {
    UiReady,
    Navigate {
        input: String,
    },
    OpenUrl {
        url: String,
        new_tab: bool,
    },
    OmniboxQuery {
        query: String,
        left: f64,
        width: f64,
    },
    OmniboxKey {
        key: String,
    },
    OmniboxClose,
    SelectSuggestion {
        index: usize,
    },
    ToggleSiteInfoPopover {
        left: f64,
    },
    CloseOverlay,
    CreateTabGroup {
        tab_id: u64,
        name: String,
        color: String,
    },
    AssignTabToGroup {
        tab_id: u64,
        group_id: Option<String>,
    },
    ToggleTabGroupCollapse {
        group_id: String,
    },
    RenameTabGroup {
        group_id: String,
        name: String,
        color: Option<String>,
    },
    UngroupTabGroup {
        group_id: String,
    },
    CloseTabGroup {
        group_id: String,
    },
    HibernateAllInactive,
    RefreshMemoryStats,
    AddCustomSearchEngine {
        keyword: String,
        name: String,
        url_template: String,
    },
    RemoveCustomSearchEngine {
        keyword: String,
    },
    CompleteOnboarding,
    SaveGistSyncConfig {
        token: String,
        gist_id: String,
    },
    ClearGistSyncConfig,
    SyncGistPush {
        passphrase: String,
    },
    SyncGistPull {
        passphrase: String,
    },
    NewTab {
        url: Option<String>,
        private: Option<bool>,
    },
    NewWindow {
        url: Option<String>,
    },
    MoveTabToNewWindow {
        id: u64,
    },
    LaunchPwaCurrent,
    InstallPwaShortcutCurrent,
    OpenCurrentInFullBrowser,
    SwitchTab {
        id: u64,
    },
    CloseTab {
        id: u64,
    },
    CloseOtherTabs {
        id: u64,
    },
    DuplicateTab {
        id: u64,
    },
    TogglePinTab {
        id: u64,
    },
    ToggleMuteTab {
        id: u64,
    },
    HibernateTab {
        id: u64,
    },
    ReorderTab {
        from_id: u64,
        to_id: u64,
    },
    ReopenClosedTab,
    ReopenClosedUrl {
        url: String,
    },
    OpenTabContext {
        id: Option<u64>,
    },
    ToggleAudioHub,
    ToggleSplitView {
        target_tab_id: Option<u64>,
    },
    SwapSplitView,
    CloseSplitView,
    SetSplitRatio {
        ratio: f64,
    },
    ResizeSplitDelta {
        dx: f64,
    },
    SwitchWorkspace {
        id: String,
    },
    CreateWorkspace {
        name: String,
        accent: String,
        isolated: bool,
    },
    ToggleWorkspaceIsolation {
        id: String,
    },
    DeleteWorkspace {
        id: String,
    },
    MoveTabToWorkspace {
        tab_id: u64,
        workspace_id: String,
    },
    SetLayoutMode {
        mode: String,
    },
    ToggleVerticalCollapsed,
    GoBack,
    GoForward,
    Reload,
    Stop,
    GoHome,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleReaderMode,
    ToggleForceDark,
    TogglePip,
    CaptureScreenshot,
    CopyCleanUrl,
    TranslateCurrentPage,
    PrintPage,
    ToggleFullscreen,
    OpenDevtools,
    OpenFind,
    CloseFind,
    FindQuery {
        query: String,
        case_sensitive: bool,
        step: i32,
    },
    ToggleBookmarkCurrent,
    AddBookmark {
        title: String,
        url: String,
    },
    RemoveBookmark {
        id: String,
    },
    ImportBookmarks {
        browser_id: String,
    },
    ToggleBookmarksBar,
    RemoveHistory {
        id: String,
    },
    ClearHistory,
    PauseDownload {
        id: String,
    },
    ResumeDownload {
        id: String,
    },
    CancelDownload {
        id: String,
    },
    OpenDownload {
        path: String,
    },
    RevealDownload {
        path: String,
    },
    RemoveDownload {
        id: String,
    },
    ClearDownloads,
    AddNote {
        title: String,
        content: String,
        attach_current: bool,
    },
    RemoveNote {
        id: String,
    },
    TogglePanel {
        panel: PanelTab,
    },
    ClosePanel,
    ResizePanelDelta {
        dx: f64,
    },
    BrowseFolder {
        target: String,
    },
    BrowseBackupFile,
    ToggleShieldGlobal,
    ToggleHttpsOnly,
    ToggleShieldCosmetic,
    ToggleCookieBanners,
    ToggleAntiFingerprint,
    ToggleStripTracking,
    StartElementPicker,
    RemoveCosmeticRule {
        host: String,
        selector: String,
    },
    ClearCurrentSiteCookies,
    UpdateFilterLists,
    ToggleRestoreSession,
    ToggleLiveSearchSuggestions,
    SetAutoHibernate {
        minutes: u32,
    },
    SetPermission {
        kind: String,
        mode: PermissionMode,
    },
    ToggleSiteAllowlist {
        host: String,
    },
    RemoveAllowlistDomain {
        domain: String,
    },
    AddCustomBlockDomain {
        domain: String,
    },
    RemoveCustomBlockDomain {
        domain: String,
    },
    SetSearchEngine {
        engine: String,
    },
    ClearAllBrowsingData,
    RegisterDefaultBrowser,
    CreateWindowsShortcuts,
    VaultSetup {
        password: String,
    },
    VaultUnlock {
        password: String,
    },
    VaultLock,
    VaultAdd {
        site: String,
        username: String,
        password: String,
        notes: String,
    },
    VaultRemove {
        id: String,
    },
    VaultCopyPassword {
        id: String,
    },
    VaultAutofill {
        id: String,
    },
    QuickAutofill,
    AcceptPendingLogin,
    DismissPendingLogin,
    VaultGeneratePassword {
        length: usize,
        symbols: bool,
    },
    VaultImportCsv {
        csv: String,
    },
    VaultExportCsv,
    SetVaultAutoLock {
        minutes: u32,
    },
    AddChromiumExtension {
        folder_path: String,
    },
    ToggleChromiumExtension {
        id: String,
        enabled: bool,
    },
    RemoveChromiumExtension {
        id: String,
    },
    AddUserScript {
        name: String,
        match_pattern: String,
        code: String,
    },
    ToggleUserScript {
        id: String,
    },
    RemoveUserScript {
        id: String,
    },
    ExportEncryptedBackup {
        passphrase: String,
        use_sync_folder: bool,
    },
    ImportEncryptedBackup {
        path: String,
        passphrase: String,
    },
    SetSyncFolder {
        path: String,
    },
    CheckForUpdates,
    InstallUpdate,
    WindowDrag,
    WindowMinimize,
    WindowToggleMaximize,
    WindowClose,
}
