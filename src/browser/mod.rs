mod features;
mod ipc;
mod layout;
mod tabs;
mod ui_sync;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use http::Response;
use tao::event_loop::EventLoopProxy;
use tao::window::{Fullscreen, Window};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{
    NewWindowResponse, PageLoadEvent, PermissionKind, PermissionResponse, Rect, Theme, WebContext,
    WebView, WebViewBuilder,
};

#[cfg(target_os = "windows")]
use wry::{MemoryUsageLevel, ScrollBarStyle, WebViewBuilderExtWindows, WebViewExtWindows};

use crate::adblock::{AdBlockEngine, build_page_init_script, download_and_compile_filter_lists};
use crate::autocomplete::{
    OmniboxSuggestion, SuggestionKind, build_suggestions, fetch_web_suggestions,
};
use crate::backup::{export_encrypted_backup, import_encrypted_backup};
use crate::crash;
use crate::extensions::{ChromiumExtensionInfo, UserScriptStore};
use crate::importer::{DetectedBrowser, detect_browsers, read_chromium_bookmarks};
use crate::navigation::{
    INTERNAL_NEWTAB_DISPLAY, INTERNAL_NEWTAB_URL, INTERNAL_ORIGIN_PREFIX, INTERNAL_PROTOCOL,
    SearchEngine, extract_host, is_internal_url, percent_encode_query,
    resolve_omnibox_input_with_custom, sanitize_navigation_url, strip_tracking_params,
    to_display_url,
};
use crate::os_integration::{
    MemoryStats, backups_dir, create_pwa_desktop_shortcut, create_windows_shortcuts,
    is_registered_in_windows, register_as_windows_browser, sample_browser_memory,
    spawn_new_browser_window, spawn_pwa_window,
};
use crate::single_instance::try_forward_to_running_instance;
use crate::storage::{
    BrowserStorage, DownloadStatus, LayoutMode, PermissionMode, PermissionSettings, SavedTab,
    SessionData, TabGroup, generate_id, unix_now,
};
use crate::sync::{
    SyncPullResult, SyncPushResult, protect_token_dpapi, pull_encrypted_gist, push_encrypted_gist,
    unprotect_token_dpapi,
};
use crate::ui::{
    CHROME_HTML, ClosedTabInfo, DIVIDER_HTML, FindSnapshot, IpcCommand, NEWTAB_HTML, NewTabData,
    OVERLAY_HTML, OverlayState, PANEL_HTML, PWA_CHROME_HTML, PanelTab, PendingLoginPrompt,
    ShieldSnapshot, SiteInfoSnapshot, SyncSnapshot, TabSummary, UiState, UpdateSnapshot,
    VERTICAL_HTML, VaultEntrySummary, VaultSnapshot,
};
use crate::updater::{
    CURRENT_VERSION, RELEASES_PAGE_URL, UpdateCheckResult, UpdateInfo, check_for_updates,
    download_verify_and_install,
};
use crate::vault::{PasswordVault, generate_secure_password};
use crate::webview_hooks::{self, DownloadOpsMap};

const TOOLBAR_HEIGHT_MINIMAL: u32 = 42;
const TOOLBAR_HEIGHT_COMPACT: u32 = 44;
const TOOLBAR_HEIGHT_VERTICAL: u32 = 40;
const PWA_BAR_HEIGHT: u32 = 34;
const TAB_BAR_HEIGHT: u32 = 36;
const CONTEXT_BAR_HEIGHT: u32 = 32;
const AUDIO_BAR_HEIGHT: u32 = 32;
const LOGIN_BAR_HEIGHT: u32 = 32;
const FIND_BAR_HEIGHT: u32 = 34;
const BOOKMARKS_BAR_HEIGHT: u32 = 28;
const PANEL_WIDTH: u32 = 368;
const VERTICAL_WIDTH_EXPANDED: u32 = 224;
const VERTICAL_WIDTH_COLLAPSED: u32 = 52;
const DIVIDER_WIDTH: u32 = 6;

#[derive(Debug, Clone)]
pub enum AppLaunchMode {
    Normal {
        restore_session: bool,
        cli_urls: Vec<String>,
        smoke_test: bool,
        ephemeral_test: bool,
    },
    Pwa {
        url: String,
        title: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub enum ContextMenuAction {
    OpenLinkNewTab(String),
    OpenLinkSplit(String),
    OpenLinkPrivate(String),
    CopyCleanLink(String),
    SearchSelection(String),
    TranslateSelection(String),
    SaveSelectionToNotes(String),
    StartElementPicker,
    CaptureScreenshot,
}

#[derive(Debug, Clone, Copy)]
pub enum ShortcutAction {
    NewTab,
    NewWindow,
    NewPrivateTab,
    ReopenClosedTab,
    CloseTab,
    DuplicateTab,
    TogglePinTab,
    ToggleMuteTab,
    MoveActiveTabToNewWindow,
    OpenTaskManager,
    SwitchWorkspaceIndex(usize),
    FocusOmnibox,
    QuickAutofill,
    StartElementPicker,
    OpenCommands,
    OpenFind,
    ToggleDownloads,
    Reload,
    ToggleHistory,
    ToggleBookmark,
    ToggleBookmarksBar,
    ToggleSplitView,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    PrintPage,
    ToggleReaderMode,
    ToggleForceDark,
    TogglePiP,
    CaptureScreenshot,
    CopyCleanUrl,
    ToggleFullscreen,
    NextTab,
    PrevTab,
    JumpToTab(usize),
    GoBack,
    GoForward,
    OpenDevtools,
    EscapePressed,
}

pub enum BrowserEvent {
    Ipc(IpcCommand),
    ContextAction(ContextMenuAction),
    ExternalOpenUrls(Vec<String>),
    TabTitleChanged {
        tab_id: u64,
        title: String,
    },
    TabLoadEvent {
        tab_id: u64,
        started: bool,
        url: String,
    },
    TabNavUpdated {
        tab_id: u64,
        url: String,
        can_go_back: bool,
        can_go_forward: bool,
    },
    TabAudioChanged {
        tab_id: u64,
        playing: bool,
        muted: bool,
    },
    TabFaviconChanged {
        tab_id: u64,
        favicon: Option<String>,
    },
    RequestBlocked {
        tab_id: u64,
    },
    FindResult {
        current: usize,
        total: usize,
    },
    ReaderToggled {
        tab_id: u64,
        active: bool,
    },
    ForceDarkToggled {
        tab_id: u64,
        active: bool,
    },
    PipResult(String),
    AutofillResult(usize),
    ElementPicked {
        host: String,
        selector: String,
    },
    LoginDetected {
        host: String,
        username: String,
        password: String,
    },
    WebSuggestionsReady {
        seq: u64,
        query: String,
        suggestions: Vec<String>,
    },
    ExtensionsUpdated(Vec<ChromiumExtensionInfo>),
    NativePathPicked {
        target: String,
        path: String,
    },
    DownloadStarted {
        id: String,
        url: String,
        path: PathBuf,
        total_bytes: Option<u64>,
    },
    DownloadProgress {
        id: String,
        received: u64,
        total: Option<u64>,
    },
    DownloadStateChanged {
        id: String,
        status: DownloadStatus,
        path: Option<PathBuf>,
    },
    FilterListsUpdated(Result<(adblock::Engine, usize, usize), String>),
    ScreenshotSaved(PathBuf),
    Toast(String),
    HtmlFullscreenChanged(bool),
    OpenNewTabUrl(String),
    Shortcut(ShortcutAction),
    WarmSpareTab(u64),
    PeriodicTick,
    StartupUpdateCheck,
    UpdateCheckFinished {
        silent: bool,
        result: Result<UpdateCheckResult, String>,
    },
    UpdateInstallFinished(Result<String, String>),
    MemoryStatsReady(MemoryStats),
    GistSyncPushFinished(Result<SyncPushResult, String>),
    GistSyncPullFinished(Result<SyncPullResult, String>),
    TestTimeout,
    RequestExit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayMode {
    Suggestions,
    SiteInfo,
}

struct PendingLoginSecret {
    host: String,
    username: String,
    password: String,
}

struct SpareTab {
    webview: WebView,
    tab_id_cell: Arc<AtomicU64>,
    host_cell: Arc<RwLock<Option<String>>>,
}

struct Tab {
    id: u64,
    webview: Option<WebView>,
    is_visible: bool,
    title: String,
    url: String,
    favicon_url: Option<String>,
    host_cell: Arc<RwLock<Option<String>>>,
    workspace_id: String,
    group_id: Option<String>,
    pinned: bool,
    is_private: bool,
    loading: bool,
    can_go_back: bool,
    can_go_forward: bool,
    is_playing_audio: bool,
    is_muted: bool,
    zoom_factor: f64,
    reader_active: bool,
    force_dark_active: bool,
    last_active: Instant,
}

pub struct BrowserApp {
    window: Window,
    proxy: EventLoopProxy<BrowserEvent>,
    _web_context: WebContext,
    #[cfg(target_os = "windows")]
    shared_env: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment,
    is_pwa_mode: bool,
    pwa_title: Option<String>,
    smoke_test: bool,
    ephemeral_test: bool,
    bridge_nonce: Arc<String>,
    page_init_script: String,
    chrome_webview: WebView,
    vertical_webview: WebView,
    overlay_webview: WebView,
    divider_webview: WebView,
    panel_webview: WebView,
    tabs: Vec<Tab>,
    tab_groups: Vec<TabGroup>,
    spare_tab: Option<SpareTab>,
    spare_gen: u64,
    spare_timer_tx: Option<std::sync::mpsc::Sender<(u64, Duration)>>,
    active_tab_id: u64,
    split_tab_id: Option<u64>,
    split_ratio: f64,
    context_tab_id: Option<u64>,
    audio_hub_open: bool,
    active_workspace_id: String,
    next_tab_id: u64,
    closed_tabs: Vec<ClosedTabInfo>,
    open_panel: Option<PanelTab>,
    panel_width: f64,
    last_layout_phys: (u32, u32),
    picked_backup_path: Option<String>,
    find: FindSnapshot,
    is_fullscreen: bool,
    storage: BrowserStorage,
    adblock: Arc<RwLock<AdBlockEngine>>,
    updating_lists: bool,
    permissions: Arc<RwLock<PermissionSettings>>,
    newtab_snapshot: Arc<RwLock<NewTabData>>,
    download_ops: DownloadOpsMap,
    vault: PasswordVault,
    vault_error: Option<String>,
    generated_password: Option<String>,
    pending_login: Option<PendingLoginSecret>,
    detected_browsers: Vec<DetectedBrowser>,
    windows_registered: bool,
    update_checking: bool,
    update_installing: bool,
    available_update: Option<UpdateInfo>,
    update_status: Option<String>,
    extensions: Vec<ChromiumExtensionInfo>,
    userscripts: UserScriptStore,
    overlay_visible: bool,
    overlay_mode: OverlayMode,
    overlay_query: String,
    overlay_suggestions: Vec<OmniboxSuggestion>,
    overlay_selected: usize,
    overlay_rect: (f64, f64),
    memory_stats: MemoryStats,
    sync_busy: bool,
    sync_status: Option<String>,
    recovered_from_crash: bool,
    crash_reports_count: usize,
    live_query_seq: u64,
    pending_toast: Option<String>,
    ui_dirty: bool,
    session_dirty: bool,
    last_persisted_blocked: u64,
}

impl BrowserApp {
    pub fn new(
        window: Window,
        proxy: EventLoopProxy<BrowserEvent>,
        launch_mode: AppLaunchMode,
    ) -> wry::Result<Self> {
        let storage = BrowserStorage::load();
        let profile_dir = storage.webview_profile_dir();
        let filters_dir = storage.filters_dir();
        let extensions_dir = storage.extensions_dir();
        let data_dir = storage.data_dir();
        let mut web_context = WebContext::new(Some(profile_dir));

        let (is_pwa_mode, pwa_title, smoke_test, ephemeral_test) = match &launch_mode {
            AppLaunchMode::Pwa { title, .. } => (true, title.clone(), false, false),
            AppLaunchMode::Normal {
                smoke_test,
                ephemeral_test,
                ..
            } => (false, None, *smoke_test, *ephemeral_test),
        };

        let bridge_nonce = Arc::new(generate_bridge_nonce());
        let page_init_script = build_page_init_script(&bridge_nonce);

        let active_workspace_id = storage
            .settings
            .workspaces
            .first()
            .map(|w| w.id.clone())
            .unwrap_or_else(|| "ws_general".to_string());

        let adblock_engine = AdBlockEngine::new(&storage.settings, &filters_dir);
        let recovered_from_crash = if !is_pwa_mode && !ephemeral_test {
            crash::mark_session_running(&data_dir)
        } else {
            false
        };
        let crash_reports_count = crash::crash_reports_count(&data_dir);
        let detected_browsers = detect_browsers();
        let windows_registered = is_registered_in_windows();
        let initial_newtab = NewTabData {
            search_engine: storage.settings.search_engine,
            total_blocked: adblock_engine.total_blocked(),
            bytes_saved: adblock_engine.bytes_saved(),
            rules_count: adblock_engine.rules_count(),
            shield_enabled: adblock_engine.is_enabled(),
            https_only: storage.settings.https_only,
            anti_fingerprint: storage.settings.anti_fingerprint,
            is_private: false,
            workspace_name: "General".to_string(),
            bookmarks: storage.bookmarks.clone(),
            recent_history: storage.history.iter().take(8).cloned().collect(),
            closed_tabs: Vec::new(),
            show_onboarding: !storage.settings.onboarding_completed
                && !ephemeral_test
                && !is_pwa_mode,
            layout_mode: storage.settings.layout_mode,
            windows_registered,
            detected_browsers: detected_browsers.clone(),
            recovered_from_crash,
        };
        let last_persisted_blocked = adblock_engine.total_blocked();
        let adblock = Arc::new(RwLock::new(adblock_engine));
        let permissions = Arc::new(RwLock::new(storage.settings.permissions.clone()));
        let newtab_snapshot = Arc::new(RwLock::new(initial_newtab));
        let download_ops: DownloadOpsMap = Rc::new(RefCell::new(HashMap::new()));
        let vault = PasswordVault::new(data_dir.clone());
        let userscripts = UserScriptStore::load(data_dir);

        let (win_w, win_h) = logical_window_size(&window);
        let top_h = if is_pwa_mode {
            PWA_BAR_HEIGHT
        } else {
            let base = match storage.settings.layout_mode {
                LayoutMode::Minimal => TOOLBAR_HEIGHT_MINIMAL + TAB_BAR_HEIGHT,
                LayoutMode::Compact => TOOLBAR_HEIGHT_COMPACT,
                LayoutMode::Vertical => TOOLBAR_HEIGHT_VERTICAL,
            };
            base + if storage.settings.show_bookmarks_bar {
                BOOKMARKS_BAR_HEIGHT
            } else {
                0
            }
        };

        // 1. Top Chrome WebView (adaptive Apple Noir chrome or minimal 34px PWA bar)
        let chrome_proxy = proxy.clone();
        let chrome_builder = WebViewBuilder::new_with_web_context(&mut web_context)
            .with_bounds(Rect {
                position: LogicalPosition::new(0, 0).into(),
                size: LogicalSize::new(win_w, top_h).into(),
            })
            .with_visible(false)
            .with_focused(false)
            .with_background_color((0, 0, 0, 255))
            .with_html(if is_pwa_mode {
                PWA_CHROME_HTML
            } else {
                CHROME_HTML
            })
            .with_devtools(false)
            .with_navigation_handler(|url| url == "about:blank" || url.starts_with("data:"))
            .with_ipc_handler(move |req| {
                if let Ok(cmd) = serde_json::from_str::<IpcCommand>(req.body()) {
                    let _ = chrome_proxy.send_event(BrowserEvent::Ipc(cmd));
                }
            });

        #[cfg(target_os = "windows")]
        let chrome_builder = chrome_builder
            .with_theme(Theme::Dark)
            .with_browser_extensions_enabled(true)
            .with_extensions_path(extensions_dir)
            .with_default_context_menus(false)
            .with_browser_accelerator_keys(false)
            .with_scroll_bar_style(ScrollBarStyle::FluentOverlay);

        let chrome_webview = chrome_builder.build_as_child(&window)?;
        webview_hooks::enable_clip_siblings(&chrome_webview);
        webview_hooks::attach_accelerator_hooks(&chrome_webview, proxy.clone());

        #[cfg(target_os = "windows")]
        let shared_env = chrome_webview.environment();

        // 2. Left Vertical Tab Sidebar WebView (Arc / Zen mode)
        let vert_w = if storage.settings.vertical_collapsed {
            VERTICAL_WIDTH_COLLAPSED
        } else {
            VERTICAL_WIDTH_EXPANDED
        };
        let vert_proxy = proxy.clone();
        let vert_builder = WebViewBuilder::new()
            .with_bounds(Rect {
                position: LogicalPosition::new(0, top_h).into(),
                size: LogicalSize::new(vert_w, win_h.saturating_sub(top_h)).into(),
            })
            .with_visible(false)
            .with_focused(false)
            .with_background_color((5, 5, 7, 255))
            .with_html(VERTICAL_HTML)
            .with_devtools(false)
            .with_navigation_handler(|url| url == "about:blank" || url.starts_with("data:"))
            .with_ipc_handler(move |req| {
                if let Ok(cmd) = serde_json::from_str::<IpcCommand>(req.body()) {
                    let _ = vert_proxy.send_event(BrowserEvent::Ipc(cmd));
                }
            });

        #[cfg(target_os = "windows")]
        let vert_builder = vert_builder
            .with_environment(shared_env.clone())
            .with_theme(Theme::Dark)
            .with_default_context_menus(false)
            .with_browser_accelerator_keys(false)
            .with_scroll_bar_style(ScrollBarStyle::FluentOverlay);

        let vertical_webview = vert_builder.build_as_child(&window)?;
        webview_hooks::enable_clip_siblings(&vertical_webview);
        webview_hooks::attach_accelerator_hooks(&vertical_webview, proxy.clone());

        // 3. Floating Omnibox Autocomplete Overlay WebView
        let overlay_proxy = proxy.clone();
        let overlay_builder = WebViewBuilder::new()
            .with_bounds(Rect {
                position: LogicalPosition::new(280, 40).into(),
                size: LogicalSize::new(500, 120).into(),
            })
            .with_visible(false)
            .with_focused(false)
            .with_background_color((13, 13, 17, 255))
            .with_html(OVERLAY_HTML)
            .with_devtools(false)
            .with_navigation_handler(|url| url == "about:blank" || url.starts_with("data:"))
            .with_ipc_handler(move |req| {
                if let Ok(cmd) = serde_json::from_str::<IpcCommand>(req.body()) {
                    let _ = overlay_proxy.send_event(BrowserEvent::Ipc(cmd));
                }
            });

        #[cfg(target_os = "windows")]
        let overlay_builder = overlay_builder
            .with_environment(shared_env.clone())
            .with_theme(Theme::Dark)
            .with_default_context_menus(false)
            .with_browser_accelerator_keys(false)
            .with_scroll_bar_style(ScrollBarStyle::FluentOverlay);

        let overlay_webview = overlay_builder.build_as_child(&window)?;
        webview_hooks::enable_clip_siblings(&overlay_webview);
        webview_hooks::attach_accelerator_hooks(&overlay_webview, proxy.clone());

        // 4. Draggable Split View Divider WebView
        let div_proxy = proxy.clone();
        let div_builder = WebViewBuilder::new()
            .with_bounds(Rect {
                position: LogicalPosition::new(win_w / 2, top_h).into(),
                size: LogicalSize::new(DIVIDER_WIDTH, win_h.saturating_sub(top_h)).into(),
            })
            .with_visible(false)
            .with_focused(false)
            .with_background_color((5, 5, 7, 255))
            .with_html(DIVIDER_HTML)
            .with_devtools(false)
            .with_navigation_handler(|url| url == "about:blank" || url.starts_with("data:"))
            .with_ipc_handler(move |req| {
                if let Ok(cmd) = serde_json::from_str::<IpcCommand>(req.body()) {
                    let _ = div_proxy.send_event(BrowserEvent::Ipc(cmd));
                }
            });

        #[cfg(target_os = "windows")]
        let div_builder = div_builder
            .with_environment(shared_env.clone())
            .with_theme(Theme::Dark)
            .with_default_context_menus(false)
            .with_browser_accelerator_keys(false);

        let divider_webview = div_builder.build_as_child(&window)?;
        webview_hooks::enable_clip_siblings(&divider_webview);
        webview_hooks::attach_accelerator_hooks(&divider_webview, proxy.clone());

        // 5. Right Sidebar Panel WebView
        let panel_proxy = proxy.clone();
        let panel_builder = WebViewBuilder::new()
            .with_bounds(Rect {
                position: LogicalPosition::new(win_w.saturating_sub(PANEL_WIDTH), top_h).into(),
                size: LogicalSize::new(PANEL_WIDTH, win_h.saturating_sub(top_h)).into(),
            })
            .with_visible(false)
            .with_focused(false)
            .with_background_color((7, 7, 9, 255))
            .with_html(PANEL_HTML)
            .with_devtools(false)
            .with_navigation_handler(|url| url == "about:blank" || url.starts_with("data:"))
            .with_ipc_handler(move |req| {
                if let Ok(cmd) = serde_json::from_str::<IpcCommand>(req.body()) {
                    let _ = panel_proxy.send_event(BrowserEvent::Ipc(cmd));
                }
            });

        #[cfg(target_os = "windows")]
        let panel_builder = panel_builder
            .with_environment(shared_env.clone())
            .with_theme(Theme::Dark)
            .with_default_context_menus(false)
            .with_browser_accelerator_keys(false)
            .with_scroll_bar_style(ScrollBarStyle::FluentOverlay);

        let panel_webview = panel_builder.build_as_child(&window)?;
        webview_hooks::enable_clip_siblings(&panel_webview);
        webview_hooks::attach_accelerator_hooks(&panel_webview, proxy.clone());

        let spare_timer_tx = if !is_pwa_mode && !smoke_test {
            let (tx, rx) = std::sync::mpsc::channel::<(u64, Duration)>();
            let spare_proxy = proxy.clone();
            std::thread::spawn(move || {
                let mut pending: Option<(u64, Instant)> = None;
                loop {
                    match pending {
                        None => match rx.recv() {
                            Ok((gen_id, delay)) => {
                                pending = Some((gen_id, Instant::now() + delay));
                            }
                            Err(_) => break,
                        },
                        Some((gen_id, deadline)) => {
                            let rem = deadline.saturating_duration_since(Instant::now());
                            if rem.is_zero() {
                                pending = None;
                                if spare_proxy
                                    .send_event(BrowserEvent::WarmSpareTab(gen_id))
                                    .is_err()
                                {
                                    break;
                                }
                            } else {
                                match rx.recv_timeout(rem) {
                                    Ok((new_gen, delay)) => {
                                        pending = Some((new_gen, Instant::now() + delay));
                                    }
                                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                        pending = None;
                                        if spare_proxy
                                            .send_event(BrowserEvent::WarmSpareTab(gen_id))
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                                }
                            }
                        }
                    }
                }
            });
            Some(tx)
        } else {
            None
        };

        let mut app = Self {
            window,
            proxy,
            _web_context: web_context,
            #[cfg(target_os = "windows")]
            shared_env,
            is_pwa_mode,
            pwa_title,
            smoke_test,
            ephemeral_test,
            bridge_nonce,
            page_init_script,
            chrome_webview,
            vertical_webview,
            overlay_webview,
            divider_webview,
            panel_webview,
            tabs: Vec::new(),
            tab_groups: Vec::new(),
            spare_tab: None,
            spare_gen: 0,
            spare_timer_tx,
            active_tab_id: 0,
            split_tab_id: None,
            split_ratio: 0.5,
            context_tab_id: None,
            audio_hub_open: false,
            active_workspace_id,
            next_tab_id: 1,
            closed_tabs: Vec::new(),
            open_panel: None,
            panel_width: PANEL_WIDTH as f64,
            last_layout_phys: (0, 0),
            picked_backup_path: None,
            find: FindSnapshot {
                open: false,
                query: String::new(),
                case_sensitive: false,
                current: 0,
                total: 0,
            },
            is_fullscreen: false,
            storage,
            adblock,
            updating_lists: false,
            permissions,
            newtab_snapshot,
            download_ops,
            vault,
            vault_error: None,
            generated_password: None,
            pending_login: None,
            detected_browsers,
            windows_registered,
            update_checking: false,
            update_installing: false,
            available_update: None,
            update_status: None,
            extensions: Vec::new(),
            userscripts,
            overlay_visible: false,
            overlay_mode: OverlayMode::Suggestions,
            overlay_query: String::new(),
            overlay_suggestions: Vec::new(),
            overlay_selected: 0,
            overlay_rect: (280.0, 500.0),
            memory_stats: MemoryStats::default(),
            sync_busy: false,
            sync_status: None,
            recovered_from_crash,
            crash_reports_count,
            live_query_seq: 0,
            pending_toast: None,
            ui_dirty: true,
            session_dirty: false,
            last_persisted_blocked,
        };

        match launch_mode {
            AppLaunchMode::Pwa { url, .. } => {
                app.create_tab(&url, true, false)?;
            }
            AppLaunchMode::Normal {
                restore_session,
                cli_urls,
                ..
            } => {
                let mut restored = false;
                if restore_session && let Some(session) = app.storage.load_session() {
                    app.tab_groups = session.groups;
                    if app
                        .storage
                        .settings
                        .workspaces
                        .iter()
                        .any(|w| w.id == session.active_workspace_id)
                    {
                        app.active_workspace_id = session.active_workspace_id;
                    }
                    let restored_list: Vec<&SavedTab> = session.tabs.iter().take(20).collect();
                    let active_idx = session
                        .active_index
                        .min(restored_list.len().saturating_sub(1));
                    let mut active_id_to_select = None;

                    for (idx, saved) in restored_list.into_iter().enumerate() {
                        let target_url = if is_internal_url(&saved.url) {
                            INTERNAL_NEWTAB_URL.to_string()
                        } else {
                            saved.url.clone()
                        };
                        let ws_id = if app
                            .storage
                            .settings
                            .workspaces
                            .iter()
                            .any(|w| w.id == saved.workspace_id)
                        {
                            saved.workspace_id.clone()
                        } else {
                            app.active_workspace_id.clone()
                        };
                        let valid_group = saved
                            .group_id
                            .clone()
                            .filter(|gid| app.tab_groups.iter().any(|g| g.id == *gid));

                        // If recovering from a crash, keep external tabs safely hibernated on startup
                        // so a crashing webpage cannot trigger a crash loop!
                        if idx == active_idx && cli_urls.is_empty() && !recovered_from_crash {
                            // Only wake the active tab eagerly on startup!
                            if let Ok(id) =
                                app.create_tab_full(&target_url, false, false, saved.pinned, ws_id)
                            {
                                if let Some(t) = app.tabs.iter_mut().find(|t| t.id == id) {
                                    t.group_id = valid_group;
                                }
                                active_id_to_select = Some(id);
                                restored = true;
                            }
                        } else {
                            // Restore background session tabs lazily hibernated (instant startup, low RAM)
                            let id = app.create_hibernated_tab(
                                &target_url,
                                &saved.title,
                                saved.pinned,
                                ws_id,
                            );
                            if let Some(t) = app.tabs.iter_mut().find(|t| t.id == id) {
                                t.group_id = valid_group;
                            }
                            if idx == active_idx {
                                active_id_to_select = Some(id);
                            }
                            restored = true;
                        }
                    }
                    app.prune_empty_tab_groups();
                    if recovered_from_crash && restored && cli_urls.is_empty() {
                        // Open a clean New Tab active while all crashed session tabs wait hibernated
                        let _ = app.create_tab(INTERNAL_NEWTAB_URL, true, false);
                        app.show_toast(
                            "Sesión anterior recuperada (pestañas hibernadas para evitar fallos)",
                        );
                    } else if let Some(id) =
                        active_id_to_select.or_else(|| app.tabs.first().map(|t| t.id))
                        && cli_urls.is_empty()
                    {
                        app.switch_tab(id);
                    }
                }

                for (i, cli_url) in cli_urls.iter().enumerate() {
                    let activate = i == cli_urls.len().saturating_sub(1);
                    if app.create_tab(cli_url, activate, false).is_ok() {
                        restored = true;
                    }
                }

                if !restored {
                    app.create_tab(INTERNAL_NEWTAB_URL, true, false)?;
                }
            }
        }

        // Load installed Chromium MV3 extension list and initial memory stats asynchronously
        webview_hooks::list_browser_extensions(&app.chrome_webview, app.proxy.clone());
        if !app.smoke_test && !app.ephemeral_test {
            app.refresh_memory_stats_async();
        }

        app.relayout();
        app.schedule_spare_tab_warm(Duration::from_millis(900));
        Ok(app)
    }

    pub fn reveal_window(&self) {
        self.window.set_visible(true);
    }

    pub fn notify_post_update(&mut self) {
        self.show_toast(format!("Ferrum actualizado a v{CURRENT_VERSION}"));
    }

    pub fn handle_resize(&mut self, phys_override: Option<(u32, u32)>) {
        self.close_overlay();
        self.relayout_with_physical(phys_override);
        self.ui_dirty = true;
    }

    pub fn handle_window_focused(&mut self) {
        if self.window.is_minimized() {
            return;
        }
        let sz = self.window.inner_size();
        if sz.width > 0 && sz.height > 0 && (sz.width, sz.height) != self.last_layout_phys {
            self.handle_resize(Some((sz.width, sz.height)));
        }
    }

    pub fn handle_window_moved(&self) {
        webview_hooks::notify_parent_moved(&self.chrome_webview);
        webview_hooks::notify_parent_moved(&self.vertical_webview);
        webview_hooks::notify_parent_moved(&self.panel_webview);
        if let Some(wv) = self.active_webview() {
            webview_hooks::notify_parent_moved(wv);
        }
    }

    pub fn tabs_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab_id(&self) -> u64 {
        self.active_tab_id
    }

    pub fn flush_ui_if_dirty(&mut self) {
        if self.session_dirty {
            self.session_dirty = false;
            self.save_session_now();
        }
        if !self.ui_dirty {
            return;
        }
        self.ui_dirty = false;
        self.refresh_newtab_snapshot(false);
        self.persist_blocked_counter_if_needed(false);

        let toast = self.pending_toast.take();
        // Deliver non-intrusive bottom-right toast inside the active tab (closed Shadow DOM)
        // so it never overlaps the omnibox or tab bar!
        let chrome_toast = if let Some(msg) = toast {
            if let Some(wv) = self.active_webview()
                && let Ok(msg_json) = serde_json::to_string(&msg)
            {
                let tjs =
                    format!("window.__ferrumShowToast && window.__ferrumShowToast({msg_json});");
                let _ = wv.evaluate_script(&tjs);
                None
            } else {
                Some(msg)
            }
        } else {
            None
        };

        let state = self.build_ui_state(chrome_toast);
        if let Ok(json) = serde_json::to_string(&state) {
            let js = format!("window.__ferrumUpdateState && window.__ferrumUpdateState({json});");
            let _ = self.chrome_webview.evaluate_script(&js);
            if !self.is_pwa_mode && self.storage.settings.layout_mode == LayoutMode::Vertical {
                let _ = self.vertical_webview.evaluate_script(&js);
            }
            if !self.is_pwa_mode && self.open_panel.is_some() {
                let _ = self.panel_webview.evaluate_script(&js);
            }
            if self.overlay_visible && self.overlay_mode == OverlayMode::SiteInfo {
                self.push_overlay_state();
            }
        }
    }

    fn mark_session_dirty(&mut self) {
        if !self.is_pwa_mode && !self.ephemeral_test && self.storage.settings.restore_session {
            self.session_dirty = true;
        }
    }

    fn schedule_spare_tab_warm(&mut self, delay: Duration) {
        if self.is_pwa_mode || self.smoke_test {
            return;
        }
        self.spare_gen = self.spare_gen.wrapping_add(1);
        if let Some(tx) = &self.spare_timer_tx {
            let _ = tx.send((self.spare_gen, delay));
        }
    }

    fn warm_spare_tab(&mut self) {
        if self.is_pwa_mode || self.smoke_test || self.spare_tab.is_some() || self.tabs.len() >= 12
        {
            return;
        }
        self.refresh_newtab_snapshot(false);
        let tab_id_cell = Arc::new(AtomicU64::new(0));
        let host_cell = Arc::new(RwLock::new(None));
        let bounds = self.primary_content_bounds();
        if let Ok(webview) = self.build_tab_webview(
            tab_id_cell.clone(),
            INTERNAL_NEWTAB_URL,
            false,
            "ws_general",
            host_cell.clone(),
            bounds,
        ) {
            self.spare_tab = Some(SpareTab {
                webview,
                tab_id_cell,
                host_cell,
            });
        }
    }

    fn run_periodic_maintenance(&mut self) {
        self.check_auto_hibernation();
        if self.open_panel == Some(PanelTab::Tasks) {
            self.refresh_memory_stats_async();
        }
        if self
            .vault
            .check_auto_lock(self.storage.settings.vault_auto_lock_minutes)
        {
            self.generated_password = None;
            self.show_toast("Bóveda bloqueada automáticamente por inactividad");
        }
    }

    pub fn check_auto_hibernation(&mut self) {
        if self.is_pwa_mode {
            return;
        }
        let mins = self.storage.settings.auto_hibernate_minutes;
        if mins == 0 {
            return;
        }
        let cutoff = Duration::from_secs(u64::from(mins) * 60);
        let candidates: Vec<u64> = self
            .tabs
            .iter()
            .filter(|t| {
                t.webview.is_some()
                    && t.id != self.active_tab_id
                    && self.split_tab_id != Some(t.id)
                    && !t.pinned
                    && !t.is_private
                    && !t.is_playing_audio
                    && !is_internal_url(&t.url)
                    && t.last_active.elapsed() >= cutoff
            })
            .map(|t| t.id)
            .collect();

        for id in candidates {
            self.hibernate_tab(id, true);
        }
    }

    pub fn persist_on_exit(&mut self) {
        self.vault.lock();
        if let Some(spare) = self.spare_tab.take() {
            webview_hooks::destroy_webview_smooth(spare.webview);
        }
        self.persist_blocked_counter_if_needed(true);
        if !self.is_pwa_mode {
            self.save_session_now();
        }
        if !self.is_pwa_mode && !self.ephemeral_test {
            crash::clear_session_running(&self.storage.data_dir());
        }
    }

    fn show_toast(&mut self, msg: impl Into<String>) {
        self.pending_toast = Some(msg.into());
        self.ui_dirty = true;
    }

    pub fn handle_event(&mut self, event: BrowserEvent) {
        match event {
            BrowserEvent::Ipc(cmd) => self.handle_ipc(cmd),
            BrowserEvent::ContextAction(act) => self.handle_context_action(act),
            BrowserEvent::ExternalOpenUrls(urls) => {
                webview_hooks::bring_window_to_foreground(&self.window);
                if urls.is_empty() {
                    return;
                }
                for url in urls {
                    let clean = sanitize_navigation_url(
                        &url,
                        self.storage.settings.https_only,
                        self.storage.settings.strip_tracking_params,
                    );
                    let _ = self.create_tab(&clean, true, false);
                }
            }
            BrowserEvent::TabTitleChanged { tab_id, title } => {
                let mut history_update = None;
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id)
                    && !title.trim().is_empty()
                {
                    tab.title = title.clone();
                    if !tab.is_private {
                        history_update = Some(tab.url.clone());
                    }
                }
                if let Some(url) = history_update {
                    self.storage.update_latest_history_title(&url, &title);
                }
                self.update_window_title();
                self.ui_dirty = true;
            }
            BrowserEvent::TabLoadEvent {
                tab_id,
                started,
                url,
            } => {
                let display = to_display_url(&url);
                let host = extract_host(&display);

                if started && let Ok(mut engine) = self.adblock.write() {
                    engine.reset_tab_stats(tab_id);
                }

                struct PageFinishScripts {
                    idx: usize,
                    cosmetic_on: bool,
                    cookie_on: bool,
                    afp_on: bool,
                    extra_css: Option<String>,
                    user_cosmetic: Option<String>,
                    host: Option<String>,
                }
                let mut apply_scripts_for: Option<PageFinishScripts> = None;
                let mut record_history_pair: Option<(String, String)> = None;

                let saved_domain_zoom = host
                    .as_deref()
                    .map(|h| self.storage.domain_zoom(h))
                    .unwrap_or(1.0);

                if let Some((idx, tab)) = self
                    .tabs
                    .iter_mut()
                    .enumerate()
                    .find(|(_, t)| t.id == tab_id)
                {
                    tab.loading = started;
                    if started {
                        tab.reader_active = false;
                        tab.force_dark_active = false;
                        if is_internal_url(&display) {
                            tab.favicon_url = None;
                        }
                    }
                    if !url.is_empty() {
                        tab.url = display.clone();
                        if let Ok(mut cell) = tab.host_cell.write() {
                            *cell = host.clone();
                        }
                        if is_internal_url(&display) {
                            tab.title = if tab.is_private {
                                "Pestaña privada".to_string()
                            } else {
                                "Nueva pestaña".to_string()
                            };
                        } else if tab.title == "Nueva pestaña"
                            || tab.title == "Pestaña privada"
                            || tab.title.is_empty()
                        {
                            tab.title = host.clone().unwrap_or_else(|| display.clone());
                        }
                    }

                    if let Some(wv) = &tab.webview {
                        let nav = webview_hooks::query_nav_state(wv);
                        tab.can_go_back = nav.can_go_back;
                        tab.can_go_forward = nav.can_go_forward;

                        // Automatically restore remembered per-domain zoom level!
                        if !is_internal_url(&tab.url)
                            && (tab.zoom_factor - saved_domain_zoom).abs() > 0.01
                        {
                            tab.zoom_factor = saved_domain_zoom;
                            webview_hooks::set_zoom_factor(wv, saved_domain_zoom);
                        }
                    }

                    if !started {
                        if !tab.is_private && !is_internal_url(&tab.url) {
                            record_history_pair = Some((tab.url.clone(), tab.title.clone()));
                        }
                        let (cosmetic_on, cookie_on, afp_on, extra_css) = self
                            .adblock
                            .read()
                            .map(|e| {
                                let allowed = host
                                    .as_deref()
                                    .map(|h| !e.is_site_allowlisted(h))
                                    .unwrap_or(true);
                                let cos = e.is_enabled() && e.is_cosmetic_enabled() && allowed;
                                let ck = e.is_enabled() && e.is_cookie_banners_blocked() && allowed;
                                let afp = e.is_enabled() && e.is_anti_fingerprint() && allowed;
                                let extra = if cos {
                                    e.url_cosmetic_css(&url, host.as_deref())
                                } else {
                                    None
                                };
                                (cos, ck, afp, extra)
                            })
                            .unwrap_or((false, false, false, None));

                        let user_cosmetic = host
                            .as_deref()
                            .and_then(|h| self.storage.custom_cosmetic_css_for_host(h));

                        apply_scripts_for = Some(PageFinishScripts {
                            idx,
                            cosmetic_on,
                            cookie_on,
                            afp_on,
                            extra_css,
                            user_cosmetic,
                            host: host.clone(),
                        });
                    }
                }

                if let Some(PageFinishScripts {
                    idx,
                    cosmetic_on,
                    cookie_on,
                    afp_on,
                    extra_css,
                    user_cosmetic: user_cos,
                    host: host_opt,
                }) = apply_scripts_for
                    && let Some(wv) = &self.tabs[idx].webview
                {
                    let mut js = format!(
                        "window.__ferrumSetCosmetic && window.__ferrumSetCosmetic({cosmetic_on});\
                         window.__ferrumSetCookieBlock && window.__ferrumSetCookieBlock({cookie_on});\
                         window.__ferrumSetAntiFp && window.__ferrumSetAntiFp({afp_on});"
                    );
                    if let Some(css) = extra_css
                        && let Ok(css_json) = serde_json::to_string(&css)
                    {
                        js.push_str(&format!(
                            "window.__ferrumInjectExtraCosmetic && window.__ferrumInjectExtraCosmetic({css_json});"
                        ));
                    }
                    let ucos_json = serde_json::to_string(user_cos.as_deref().unwrap_or(""))
                        .unwrap_or_else(|_| "\"\"".into());
                    js.push_str(&format!(
                        "window.__ferrumSetUserCosmetic && window.__ferrumSetUserCosmetic({ucos_json});"
                    ));
                    let _ = wv.evaluate_script(&js);

                    // Inject matching enabled Userscripts for this domain
                    if let Some(h) = host_opt {
                        for script in self.userscripts.matching_scripts_for_host(&h) {
                            let wrapped = format!("try {{ {} }} catch (_) {{}}", script.code);
                            let _ = wv.evaluate_script(&wrapped);
                        }
                    }
                }

                if let Some((hist_url, hist_title)) = record_history_pair {
                    self.storage.record_visit(&hist_url, &hist_title);
                }
                if !started && !self.is_pwa_mode {
                    self.save_session_now();
                }

                self.update_window_title();
                self.ui_dirty = true;
            }
            BrowserEvent::TabNavUpdated {
                tab_id,
                url,
                can_go_back,
                can_go_forward,
            } => {
                let display = to_display_url(&url);
                let mut record_spa: Option<(String, String)> = None;

                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    let url_changed = tab.url != display;
                    tab.url = display.clone();
                    tab.can_go_back = can_go_back;
                    tab.can_go_forward = can_go_forward;
                    if let Ok(mut cell) = tab.host_cell.write() {
                        *cell = extract_host(&display);
                    }
                    if is_internal_url(&display) {
                        tab.title = if tab.is_private {
                            "Pestaña privada".to_string()
                        } else {
                            "Nueva pestaña".to_string()
                        };
                    } else if url_changed && !tab.loading && !tab.is_private {
                        record_spa = Some((display, tab.title.clone()));
                    }
                }
                if let Some((u, t)) = record_spa {
                    self.storage.record_visit(&u, &t);
                }
                self.ui_dirty = true;
            }
            BrowserEvent::TabAudioChanged {
                tab_id,
                playing,
                muted,
            } => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    tab.is_playing_audio = playing;
                    tab.is_muted = muted;
                }
                if self.audio_hub_open && !self.any_audio_tab() {
                    self.audio_hub_open = false;
                    self.relayout();
                }
                self.ui_dirty = true;
            }
            BrowserEvent::TabFaviconChanged { tab_id, favicon } => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    tab.favicon_url = favicon;
                    self.ui_dirty = true;
                }
            }
            BrowserEvent::RequestBlocked { tab_id } => {
                if tab_id == self.active_tab_id || self.split_tab_id == Some(tab_id) {
                    self.ui_dirty = true;
                }
            }
            BrowserEvent::FindResult { current, total } => {
                self.find.current = current;
                self.find.total = total;
                self.ui_dirty = true;
            }
            BrowserEvent::ReaderToggled { tab_id, active } => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    tab.reader_active = active;
                    self.ui_dirty = true;
                }
            }
            BrowserEvent::ForceDarkToggled { tab_id, active } => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    tab.force_dark_active = active;
                    self.ui_dirty = true;
                }
            }
            BrowserEvent::PipResult(res) => {
                let msg = match res.as_str() {
                    "entered" => "Picture-in-Picture activado",
                    "exited" => "Picture-in-Picture cerrado",
                    "no_video" => "No se detectó video en esta página",
                    _ => "Este video no permite Picture-in-Picture",
                };
                self.show_toast(msg);
            }
            BrowserEvent::AutofillResult(count) => {
                if count > 0 {
                    self.show_toast("Credenciales completadas en la página");
                } else {
                    self.show_toast("No se detectó un formulario de inicio de sesión visible");
                }
            }
            BrowserEvent::ElementPicked { host, selector } => {
                if self.storage.add_custom_cosmetic_rule(&host, &selector) {
                    self.apply_user_cosmetic_on_active_tab();
                    self.show_toast(format!("Elemento bloqueado en {host}"));
                }
                self.ui_dirty = true;
            }
            BrowserEvent::LoginDetected {
                host,
                username,
                password,
            } => {
                if self.is_pwa_mode {
                    return;
                }
                // Skip prompt if exact credential is already stored in unlocked vault
                let already_saved = self.vault.is_unlocked()
                    && self
                        .vault
                        .find_for_host(&host)
                        .map(|c| c.username == username && c.password == password)
                        .unwrap_or(false);
                if !already_saved {
                    self.pending_login = Some(PendingLoginSecret {
                        host,
                        username,
                        password,
                    });
                    self.relayout();
                    self.ui_dirty = true;
                }
            }
            BrowserEvent::WebSuggestionsReady {
                seq,
                query,
                suggestions,
            } => {
                if seq == self.live_query_seq
                    && self.overlay_visible
                    && self.overlay_mode == OverlayMode::Suggestions
                    && self.overlay_query == query
                    && !suggestions.is_empty()
                {
                    let tab_summaries = self.build_tab_summaries_light();
                    let next = build_suggestions(
                        &query,
                        &tab_summaries,
                        self.active_tab_id,
                        &self.storage.bookmarks,
                        &self.storage.history,
                        self.storage.settings.search_engine,
                        &self.storage.settings.custom_search_engines,
                        &suggestions,
                    );
                    if !next.is_empty() {
                        let sel = self.overlay_selected.min(next.len() - 1);
                        self.overlay_suggestions = next;
                        self.overlay_selected = sel;
                        self.position_and_push_overlay();
                    }
                }
            }
            BrowserEvent::ExtensionsUpdated(list) => {
                self.extensions = list;
                self.ui_dirty = true;
            }
            BrowserEvent::NativePathPicked { target, path } => match target.as_str() {
                "extension" => {
                    webview_hooks::add_browser_extension(
                        &self.chrome_webview,
                        &path,
                        self.proxy.clone(),
                    );
                }
                "sync_folder" => {
                    self.storage.settings.sync_folder_path = Some(path);
                    self.storage.save_settings();
                    self.show_toast("Carpeta de sincronización guardada");
                }
                "backup_file" => {
                    self.picked_backup_path = Some(path);
                    self.show_toast("Respaldo seleccionado · ingresa tu clave y pulsa Importar");
                }
                _ => {}
            },
            BrowserEvent::DownloadStarted {
                id,
                url,
                path,
                total_bytes,
            } => {
                self.storage
                    .record_download_started(id, url, path, total_bytes);
                if !self.is_pwa_mode {
                    self.open_panel_to(PanelTab::Downloads);
                } else {
                    self.show_toast("Descarga iniciada");
                }
                self.ui_dirty = true;
            }
            BrowserEvent::DownloadProgress {
                id,
                received,
                total,
            } => {
                self.storage.update_download_progress(&id, received, total);
                if self.open_panel == Some(PanelTab::Downloads) {
                    self.ui_dirty = true;
                }
            }
            BrowserEvent::DownloadStateChanged { id, status, path } => {
                let completed_path = path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .or_else(|| {
                        self.storage
                            .downloads
                            .iter()
                            .find(|d| d.id == id)
                            .map(|d| d.path.clone())
                    });
                self.storage.update_download_state(&id, status, path);
                if status == DownloadStatus::Completed {
                    if completed_path
                        .as_deref()
                        .map(is_dangerous_download_path)
                        .unwrap_or(false)
                    {
                        self.show_toast(
                            "Descarga ejecutable completada · revisa el origen antes de abrir",
                        );
                    } else {
                        self.show_toast("Descarga completada");
                    }
                }
                self.ui_dirty = true;
            }
            BrowserEvent::FilterListsUpdated(result) => {
                self.updating_lists = false;
                match result {
                    Ok((engine, rules_count, sources)) => {
                        if let Ok(mut guard) = self.adblock.write() {
                            guard.install_brave_engine(engine, rules_count);
                        }
                        self.storage.settings.filter_lists_updated_at = unix_now();
                        self.storage.settings.filter_lists_rules_count = rules_count;
                        self.storage.save_settings();
                        self.show_toast(format!(
                            "Filtros actualizados: {sources} listas ({rules_count} reglas)"
                        ));
                    }
                    Err(err) => {
                        self.show_toast(err);
                    }
                }
                self.ui_dirty = true;
            }
            BrowserEvent::ScreenshotSaved(path) => {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("captura.png");
                self.show_toast(format!("Captura guardada: {name}"));
            }
            BrowserEvent::Toast(msg) => self.show_toast(msg),
            BrowserEvent::HtmlFullscreenChanged(fs) => {
                self.set_fullscreen(fs);
            }
            BrowserEvent::OpenNewTabUrl(url) => {
                let clean = sanitize_navigation_url(
                    &url,
                    self.storage.settings.https_only,
                    self.storage.settings.strip_tracking_params,
                );
                if self.is_pwa_mode {
                    self.navigate_active_tab(&clean);
                } else {
                    let is_private = self.active_tab().map(|t| t.is_private).unwrap_or(false);
                    let _ = self.create_tab(&clean, true, is_private);
                }
            }
            BrowserEvent::Shortcut(action) => self.handle_shortcut(action),
            BrowserEvent::WarmSpareTab(gen_id) => {
                if gen_id == self.spare_gen {
                    self.warm_spare_tab();
                }
            }
            BrowserEvent::PeriodicTick => self.run_periodic_maintenance(),
            BrowserEvent::StartupUpdateCheck => {
                if !self.is_pwa_mode && !self.ephemeral_test {
                    self.start_update_check(true);
                }
            }
            BrowserEvent::UpdateCheckFinished { silent, result } => {
                self.update_checking = false;
                match result {
                    Ok(UpdateCheckResult::Available(info)) => {
                        let ver = info.version.clone();
                        self.available_update = Some(info);
                        self.update_status = Some(format!(
                            "Nueva versión v{ver} disponible en GitHub (con firma SHA-256)"
                        ));
                        self.show_toast(format!(
                            "Ferrum v{ver} disponible · actualiza desde Ajustes"
                        ));
                    }
                    Ok(UpdateCheckResult::UpToDate) => {
                        self.available_update = None;
                        self.update_status =
                            Some(format!("Tienes la última versión (v{CURRENT_VERSION})"));
                        if !silent {
                            self.show_toast(format!(
                                "Ferrum v{CURRENT_VERSION} ya está actualizado"
                            ));
                        }
                    }
                    Ok(UpdateCheckResult::NoReleasesYet) => {
                        self.available_update = None;
                        if !silent {
                            self.update_status = Some(
                                "Aún no hay versiones publicadas en github.com/carrub-py/ferrum"
                                    .into(),
                            );
                            self.show_toast("Aún no hay releases publicados en GitHub");
                        }
                    }
                    Err(err) => {
                        if !silent {
                            self.update_status = Some(err.clone());
                            self.show_toast(err);
                        }
                    }
                }
                self.ui_dirty = true;
            }
            BrowserEvent::UpdateInstallFinished(result) => {
                self.update_installing = false;
                match result {
                    Ok(ver) => {
                        self.update_status =
                            Some(format!("v{ver} verificada con SHA-256 · reiniciando..."));
                        let _ = self.proxy.send_event(BrowserEvent::RequestExit);
                    }
                    Err(err) => {
                        self.update_status = Some(err.clone());
                        self.show_toast(err);
                    }
                }
                self.ui_dirty = true;
            }
            BrowserEvent::MemoryStatsReady(stats) => {
                self.memory_stats = stats;
                if self.open_panel == Some(PanelTab::Tasks) {
                    self.ui_dirty = true;
                }
            }
            BrowserEvent::GistSyncPushFinished(result) => {
                self.sync_busy = false;
                match result {
                    Ok(res) => {
                        self.storage
                            .record_gist_synced(res.gist_id.clone(), res.synced_at);
                        self.sync_status = Some(format!(
                            "Subido cifrado E2EE al Gist privado ({})",
                            &res.gist_id[..res.gist_id.len().min(10)]
                        ));
                        self.show_toast(
                            "Sincronización E2EE completada (Gist privado actualizado)",
                        );
                    }
                    Err(err) => {
                        self.sync_status = Some(err.clone());
                        self.show_toast(err);
                    }
                }
                self.ui_dirty = true;
            }
            BrowserEvent::GistSyncPullFinished(result) => {
                self.sync_busy = false;
                match result {
                    Ok(res) => {
                        self.storage.reload_from_disk();
                        self.storage.settings.gist_last_sync = res.synced_at;
                        self.storage.save_settings();
                        self.userscripts.reload();
                        self.vault.lock();
                        self.relayout();
                        self.sync_status = Some(format!(
                            "Descargados y descifrados {} módulos desde Gist privado",
                            res.restored_files
                        ));
                        self.show_toast(format!(
                            "Datos sincronizados desde GitHub Gist ({} módulos)",
                            res.restored_files
                        ));
                    }
                    Err(err) => {
                        self.sync_status = Some(err.clone());
                        self.show_toast(err);
                    }
                }
                self.ui_dirty = true;
            }
            BrowserEvent::TestTimeout | BrowserEvent::RequestExit => {}
        }
    }
}

fn is_dangerous_download_path(path: &str) -> bool {
    let lower = path.trim().to_ascii_lowercase();
    [
        ".exe", ".msi", ".bat", ".cmd", ".ps1", ".vbs", ".vbe", ".js", ".jse", ".wsf", ".scr",
        ".com", ".pif", ".hta", ".lnk", ".reg", ".dll",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

fn generate_bridge_nonce() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    let mut hex = String::with_capacity(32);
    for b in bytes {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

#[cfg(target_os = "windows")]
fn workspace_profile_name(workspace_id: &str) -> String {
    let cleaned: String = workspace_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(44)
        .collect();
    format!("ferrum_{cleaned}")
}

fn parse_js_json_object(raw: &str) -> Option<serde_json::Value> {
    if let Ok(serde_json::Value::String(inner)) = serde_json::from_str::<serde_json::Value>(raw) {
        serde_json::from_str(&inner).ok()
    } else {
        serde_json::from_str(raw).ok()
    }
}

fn open_file_in_os(path: &str) {
    webview_hooks::open_file_native(path);
}

fn reveal_file_in_os(path: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("explorer")
            .arg(format!("/select,{path}"))
            .spawn();
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = Command::new("xdg-open").arg(parent).spawn();
        }
    }
}

fn logical_window_size(window: &Window) -> (u32, u32) {
    let scale = window.scale_factor();
    let logical = window.inner_size().to_logical::<u32>(scale);
    (logical.width.max(320), logical.height.max(240))
}
