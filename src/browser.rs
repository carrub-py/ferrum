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
use crate::extensions::{ChromiumExtensionInfo, UserScriptStore};
use crate::importer::{DetectedBrowser, detect_browsers, read_chromium_bookmarks};
use crate::navigation::{
    INTERNAL_NEWTAB_DISPLAY, INTERNAL_NEWTAB_URL, INTERNAL_ORIGIN_PREFIX, INTERNAL_PROTOCOL,
    SearchEngine, extract_host, is_internal_url, percent_encode_query, resolve_omnibox_input,
    sanitize_navigation_url, strip_tracking_params, to_display_url,
};
use crate::os_integration::{
    backups_dir, create_pwa_desktop_shortcut, create_windows_shortcuts, is_registered_in_windows,
    register_as_windows_browser, spawn_new_browser_window, spawn_pwa_window,
};
use crate::single_instance::try_forward_to_running_instance;
use crate::storage::{
    BrowserStorage, DownloadStatus, LayoutMode, PermissionMode, PermissionSettings, SavedTab,
    SessionData, unix_now,
};
use crate::ui::{
    CHROME_HTML, ClosedTabInfo, DIVIDER_HTML, FindSnapshot, IpcCommand, NEWTAB_HTML, NewTabData,
    OVERLAY_HTML, OverlayState, PANEL_HTML, PWA_CHROME_HTML, PanelTab, PendingLoginPrompt,
    ShieldSnapshot, TabSummary, UiState, UpdateSnapshot, VERTICAL_HTML, VaultEntrySummary,
    VaultSnapshot,
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
    TestTimeout,
    RequestExit,
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
    overlay_query: String,
    overlay_suggestions: Vec<OmniboxSuggestion>,
    overlay_selected: usize,
    overlay_rect: (f64, f64),
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
        };
        let last_persisted_blocked = adblock_engine.total_blocked();
        let adblock = Arc::new(RwLock::new(adblock_engine));
        let permissions = Arc::new(RwLock::new(storage.settings.permissions.clone()));
        let newtab_snapshot = Arc::new(RwLock::new(initial_newtab));
        let download_ops: DownloadOpsMap = Rc::new(RefCell::new(HashMap::new()));
        let vault = PasswordVault::new(data_dir.clone());
        let userscripts = UserScriptStore::load(data_dir);
        let detected_browsers = detect_browsers();
        let windows_registered = is_registered_in_windows();

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
            overlay_query: String::new(),
            overlay_suggestions: Vec::new(),
            overlay_selected: 0,
            overlay_rect: (280.0, 500.0),
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

                        if idx == active_idx && cli_urls.is_empty() {
                            // Only wake the active tab eagerly on startup!
                            if let Ok(id) =
                                app.create_tab_full(&target_url, false, false, saved.pinned, ws_id)
                            {
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
                            if idx == active_idx {
                                active_id_to_select = Some(id);
                            }
                            restored = true;
                        }
                    }
                    if let Some(id) = active_id_to_select.or_else(|| app.tabs.first().map(|t| t.id))
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

        // Load installed Chromium MV3 extension list asynchronously
        webview_hooks::list_browser_extensions(&app.chrome_webview, app.proxy.clone());

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
            BrowserEvent::TestTimeout | BrowserEvent::RequestExit => {}
        }
    }

    fn handle_context_action(&mut self, action: ContextMenuAction) {
        match action {
            ContextMenuAction::OpenLinkNewTab(url) => {
                let clean = sanitize_navigation_url(
                    &url,
                    self.storage.settings.https_only,
                    self.storage.settings.strip_tracking_params,
                );
                let is_priv = self.active_tab().map(|t| t.is_private).unwrap_or(false);
                let _ = self.create_tab(&clean, true, is_priv);
            }
            ContextMenuAction::OpenLinkSplit(url) => {
                let clean = sanitize_navigation_url(
                    &url,
                    self.storage.settings.https_only,
                    self.storage.settings.strip_tracking_params,
                );
                let is_priv = self.active_tab().map(|t| t.is_private).unwrap_or(false);
                if let Ok(new_id) = self.create_tab(&clean, false, is_priv) {
                    self.split_tab_id = Some(new_id);
                    self.relayout();
                    self.ui_dirty = true;
                }
            }
            ContextMenuAction::OpenLinkPrivate(url) => {
                let clean = sanitize_navigation_url(
                    &url,
                    self.storage.settings.https_only,
                    self.storage.settings.strip_tracking_params,
                );
                let _ = self.create_tab(&clean, true, true);
            }
            ContextMenuAction::CopyCleanLink(url) => {
                let clean = strip_tracking_params(&url);
                if webview_hooks::copy_text_to_clipboard(&clean) {
                    self.show_toast("Enlace limpio copiado al portapapeles");
                }
            }
            ContextMenuAction::SearchSelection(text) => {
                let target = self.storage.settings.search_engine.search_url(&text);
                let is_priv = self.active_tab().map(|t| t.is_private).unwrap_or(false);
                let _ = self.create_tab(&target, true, is_priv);
            }
            ContextMenuAction::SaveSelectionToNotes(text) => {
                let (src_url, src_title) = self
                    .active_tab()
                    .filter(|t| !is_internal_url(&t.url))
                    .map(|t| (Some(t.url.clone()), Some(t.title.clone())))
                    .unwrap_or((None, None));
                let title = src_title
                    .clone()
                    .unwrap_or_else(|| "Cita guardada".to_string());
                self.storage.add_note(title, text, src_url, src_title);
                self.show_toast("Cita guardada en Notas de Ferrum");
                self.ui_dirty = true;
            }
            ContextMenuAction::StartElementPicker => self.start_element_picker(),
            ContextMenuAction::CaptureScreenshot => self.capture_screenshot(),
        }
    }

    fn handle_shortcut(&mut self, action: ShortcutAction) {
        if self.is_pwa_mode {
            match action {
                ShortcutAction::Reload => {
                    if let Some(wv) = self.active_webview() {
                        let _ = wv.reload();
                    }
                }
                ShortcutAction::GoBack => {
                    if let Some(wv) = self.active_webview() {
                        webview_hooks::go_back(wv);
                    }
                }
                ShortcutAction::GoForward => {
                    if let Some(wv) = self.active_webview() {
                        webview_hooks::go_forward(wv);
                    }
                }
                ShortcutAction::ZoomIn => self.adjust_zoom(0.1),
                ShortcutAction::ZoomOut => self.adjust_zoom(-0.1),
                ShortcutAction::ZoomReset => self.set_zoom(1.0),
                ShortcutAction::ToggleFullscreen => self.set_fullscreen(!self.is_fullscreen),
                ShortcutAction::OpenDevtools => {
                    if let Some(wv) = self.active_webview() {
                        wv.open_devtools();
                    }
                }
                ShortcutAction::CloseTab => {
                    let _ = self.proxy.send_event(BrowserEvent::RequestExit);
                }
                ShortcutAction::CopyCleanUrl => self.copy_clean_url(),
                ShortcutAction::CaptureScreenshot => self.capture_screenshot(),
                ShortcutAction::TogglePiP => self.toggle_pip(),
                ShortcutAction::QuickAutofill => self.quick_autofill(),
                _ => {}
            }
            return;
        }

        match action {
            ShortcutAction::NewTab => {
                self.close_overlay();
                let _ = self.create_tab(INTERNAL_NEWTAB_URL, true, false);
                self.focus_omnibox();
            }
            ShortcutAction::NewWindow => {
                if spawn_new_browser_window(None) {
                    self.show_toast("Abriendo nueva ventana...");
                }
            }
            ShortcutAction::NewPrivateTab => {
                self.close_overlay();
                let _ = self.create_tab(INTERNAL_NEWTAB_URL, true, true);
                self.focus_omnibox();
            }
            ShortcutAction::ReopenClosedTab => {
                if let Some(info) = self.closed_tabs.pop() {
                    let target = if is_internal_url(&info.url) {
                        INTERNAL_NEWTAB_URL.to_string()
                    } else {
                        info.url
                    };
                    let _ = self.create_tab(&target, true, false);
                }
            }
            ShortcutAction::CloseTab => self.close_tab(self.active_tab_id),
            ShortcutAction::DuplicateTab => self.duplicate_tab(self.active_tab_id),
            ShortcutAction::TogglePinTab => self.toggle_pin_tab(self.active_tab_id),
            ShortcutAction::ToggleMuteTab => self.toggle_mute_tab(self.active_tab_id),
            ShortcutAction::FocusOmnibox => self.focus_omnibox(),
            ShortcutAction::QuickAutofill => self.quick_autofill(),
            ShortcutAction::StartElementPicker => self.start_element_picker(),
            ShortcutAction::OpenCommands => {
                self.close_overlay();
                self.open_panel_to(PanelTab::Commands);
                let _ = self.panel_webview.focus();
                let _ = self.panel_webview.evaluate_script(
                    "setTimeout(() => window.__ferrumFocusCommands && window.__ferrumFocusCommands(), 30);",
                );
            }
            ShortcutAction::OpenFind => self.open_find(),
            ShortcutAction::ToggleDownloads => self.toggle_panel(PanelTab::Downloads),
            ShortcutAction::Reload => {
                if let Some(wv) = self.active_webview() {
                    let _ = wv.reload();
                }
            }
            ShortcutAction::ToggleHistory => self.toggle_panel(PanelTab::History),
            ShortcutAction::ToggleBookmark => self.toggle_bookmark_current(),
            ShortcutAction::ToggleBookmarksBar => {
                self.storage.settings.show_bookmarks_bar =
                    !self.storage.settings.show_bookmarks_bar;
                self.storage.save_settings();
                self.relayout();
                self.ui_dirty = true;
            }
            ShortcutAction::ToggleSplitView => self.toggle_split_view(None),
            ShortcutAction::ZoomIn => self.adjust_zoom(0.1),
            ShortcutAction::ZoomOut => self.adjust_zoom(-0.1),
            ShortcutAction::ZoomReset => self.set_zoom(1.0),
            ShortcutAction::PrintPage => {
                if let Some(wv) = self.active_webview() {
                    let _ = wv.print();
                }
            }
            ShortcutAction::ToggleReaderMode => self.toggle_reader_mode(),
            ShortcutAction::ToggleForceDark => self.toggle_force_dark(),
            ShortcutAction::TogglePiP => self.toggle_pip(),
            ShortcutAction::CaptureScreenshot => self.capture_screenshot(),
            ShortcutAction::CopyCleanUrl => self.copy_clean_url(),
            ShortcutAction::ToggleFullscreen => self.set_fullscreen(!self.is_fullscreen),
            ShortcutAction::NextTab => self.cycle_tab(1),
            ShortcutAction::PrevTab => self.cycle_tab(-1),
            ShortcutAction::JumpToTab(idx) => self.jump_to_workspace_tab(idx),
            ShortcutAction::GoBack => {
                if let Some(wv) = self.active_webview() {
                    webview_hooks::go_back(wv);
                }
            }
            ShortcutAction::GoForward => {
                if let Some(wv) = self.active_webview() {
                    webview_hooks::go_forward(wv);
                }
            }
            ShortcutAction::OpenDevtools => {
                if let Some(wv) = self.active_webview() {
                    wv.open_devtools();
                }
            }
            ShortcutAction::EscapePressed => {
                if self.overlay_visible {
                    self.close_overlay();
                } else if self.find.open {
                    self.close_find();
                } else if self.context_tab_id.is_some() {
                    self.context_tab_id = None;
                    self.relayout();
                    self.ui_dirty = true;
                } else if self.audio_hub_open {
                    self.audio_hub_open = false;
                    self.relayout();
                    self.ui_dirty = true;
                } else if self.is_fullscreen {
                    self.set_fullscreen(false);
                }
            }
        }
    }

    fn handle_ipc(&mut self, cmd: IpcCommand) {
        match cmd {
            IpcCommand::UiReady => self.ui_dirty = true,
            IpcCommand::Navigate { input } => {
                self.close_overlay();
                let target = resolve_omnibox_input(
                    &input,
                    self.storage.settings.search_engine,
                    self.storage.settings.https_only,
                    self.storage.settings.strip_tracking_params,
                );
                self.navigate_active_tab(&target);
            }
            IpcCommand::OpenUrl { url, new_tab } => {
                self.close_overlay();
                let target = if is_internal_url(&url) {
                    INTERNAL_NEWTAB_URL.to_string()
                } else {
                    sanitize_navigation_url(
                        &url,
                        self.storage.settings.https_only,
                        self.storage.settings.strip_tracking_params,
                    )
                };
                if new_tab {
                    let is_private = self.active_tab().map(|t| t.is_private).unwrap_or(false);
                    let _ = self.create_tab(&target, true, is_private);
                } else {
                    self.navigate_active_tab(&target);
                }
            }
            IpcCommand::OmniboxQuery { query, left, width } => {
                self.handle_omnibox_query(query, left, width);
            }
            IpcCommand::OmniboxKey { key } => {
                self.handle_omnibox_key(&key);
            }
            IpcCommand::OmniboxClose => {
                self.close_overlay();
            }
            IpcCommand::SelectSuggestion { index } => {
                self.execute_suggestion(index);
            }
            IpcCommand::NewTab { url, private } => {
                self.close_overlay();
                let target = url.unwrap_or_else(|| INTERNAL_NEWTAB_URL.to_string());
                let is_priv = private.unwrap_or(false);
                let _ = self.create_tab(&target, true, is_priv);
                if target == INTERNAL_NEWTAB_URL {
                    self.focus_omnibox();
                }
            }
            IpcCommand::NewWindow { url } => {
                if spawn_new_browser_window(url.as_deref()) {
                    self.show_toast("Nueva ventana iniciada");
                }
            }
            IpcCommand::MoveTabToNewWindow { id } => {
                if let Some(url) = self.tabs.iter().find(|t| t.id == id).map(|t| t.url.clone()) {
                    let target = if is_internal_url(&url) {
                        None
                    } else {
                        Some(url.as_str())
                    };
                    if spawn_new_browser_window(target) {
                        self.context_tab_id = None;
                        if self.tabs.len() > 1 {
                            self.close_tab(id);
                        }
                    }
                }
            }
            IpcCommand::LaunchPwaCurrent => {
                if let Some((url, title)) = self
                    .active_tab()
                    .filter(|t| !is_internal_url(&t.url))
                    .map(|t| (t.url.clone(), t.title.clone()))
                {
                    if spawn_pwa_window(&url, Some(&title)) {
                        self.show_toast(format!("Abriendo «{title}» como Web App"));
                    }
                } else {
                    self.show_toast("Abre un sitio web para lanzarlo como Web App");
                }
            }
            IpcCommand::InstallPwaShortcutCurrent => {
                if let Some((url, title)) = self
                    .active_tab()
                    .filter(|t| !is_internal_url(&t.url))
                    .map(|t| (t.url.clone(), t.title.clone()))
                {
                    match create_pwa_desktop_shortcut(&title, &url) {
                        Ok(msg) => self.show_toast(msg),
                        Err(e) => self.show_toast(e),
                    }
                } else {
                    self.show_toast("Abre un sitio web para instalarlo como App");
                }
            }
            IpcCommand::OpenCurrentInFullBrowser => {
                if let Some(url) = self.active_tab().map(|t| t.url.clone()) {
                    let list = vec![url.clone()];
                    if !try_forward_to_running_instance(&list) {
                        spawn_new_browser_window(Some(&url));
                    }
                }
            }
            IpcCommand::SwitchTab { id } => self.switch_tab(id),
            IpcCommand::CloseTab { id } => self.close_tab(id),
            IpcCommand::CloseOtherTabs { id } => self.close_other_tabs(id),
            IpcCommand::DuplicateTab { id } => self.duplicate_tab(id),
            IpcCommand::TogglePinTab { id } => self.toggle_pin_tab(id),
            IpcCommand::ToggleMuteTab { id } => self.toggle_mute_tab(id),
            IpcCommand::HibernateTab { id } => {
                self.context_tab_id = None;
                self.hibernate_tab(id, false);
                self.relayout();
            }
            IpcCommand::ReorderTab { from_id, to_id } => self.reorder_tab(from_id, to_id),
            IpcCommand::ReopenClosedTab => self.handle_shortcut(ShortcutAction::ReopenClosedTab),
            IpcCommand::ReopenClosedUrl { url } => {
                self.closed_tabs.retain(|c| c.url != url);
                let _ = self.create_tab(&url, true, false);
            }
            IpcCommand::OpenTabContext { id } => {
                self.close_overlay();
                self.context_tab_id = id;
                self.relayout();
                self.ui_dirty = true;
            }
            IpcCommand::ToggleAudioHub => {
                self.close_overlay();
                self.audio_hub_open = !self.audio_hub_open && self.any_audio_tab();
                self.relayout();
                self.ui_dirty = true;
            }
            IpcCommand::ToggleSplitView { target_tab_id } => self.toggle_split_view(target_tab_id),
            IpcCommand::SwapSplitView => {
                if let Some(sec) = self.split_tab_id {
                    let prev_active = self.active_tab_id;
                    self.active_tab_id = sec;
                    self.split_tab_id = Some(prev_active);
                    self.context_tab_id = None;
                    self.relayout();
                    self.update_window_title();
                    self.ui_dirty = true;
                }
            }
            IpcCommand::CloseSplitView => {
                if self.split_tab_id.take().is_some() {
                    self.relayout();
                    self.ui_dirty = true;
                }
            }
            IpcCommand::SetSplitRatio { ratio } => {
                self.split_ratio = ratio.clamp(0.22, 0.78);
                self.relayout();
                self.ui_dirty = true;
            }
            IpcCommand::ResizeSplitDelta { dx } => {
                if self.split_tab_id.is_some() && !self.is_fullscreen {
                    let content_w = self.content_area_width().max(240) as f64;
                    self.split_ratio = (self.split_ratio + dx / content_w).clamp(0.22, 0.78);
                    self.relayout();
                }
            }
            IpcCommand::SwitchWorkspace { id } => self.switch_workspace(&id),
            IpcCommand::CreateWorkspace {
                name,
                accent,
                isolated,
            } => {
                let ws = self.storage.add_workspace(&name, &accent, isolated);
                self.show_toast(if isolated {
                    format!("Espacio aislado «{}» creado", ws.name)
                } else {
                    format!("Espacio «{}» creado", ws.name)
                });
                self.switch_workspace(&ws.id);
            }
            IpcCommand::ToggleWorkspaceIsolation { id } => {
                let state = self.storage.toggle_workspace_isolation(&id);
                self.show_toast(if state {
                    "Contenedor aislado activado para nuevas pestañas de este espacio"
                } else {
                    "Contenedor aislado desactivado"
                });
                self.ui_dirty = true;
            }
            IpcCommand::DeleteWorkspace { id } => {
                if self.storage.remove_workspace(&id) {
                    for tab in &mut self.tabs {
                        if tab.workspace_id == id {
                            tab.workspace_id = "ws_general".to_string();
                        }
                    }
                    if self.active_workspace_id == id {
                        self.active_workspace_id = "ws_general".to_string();
                    }
                    self.save_session_now();
                    self.ui_dirty = true;
                }
            }
            IpcCommand::MoveTabToWorkspace {
                tab_id,
                workspace_id,
            } => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    tab.workspace_id = workspace_id;
                }
                self.context_tab_id = None;
                self.save_session_now();
                self.relayout();
                self.ui_dirty = true;
            }
            IpcCommand::SetLayoutMode { mode } => {
                self.close_overlay();
                self.storage.settings.layout_mode = LayoutMode::from_id(&mode);
                self.storage.save_settings();
                self.relayout();
                self.ui_dirty = true;
            }
            IpcCommand::ToggleVerticalCollapsed => {
                self.storage.settings.vertical_collapsed =
                    !self.storage.settings.vertical_collapsed;
                self.storage.save_settings();
                self.relayout();
                self.ui_dirty = true;
            }
            IpcCommand::GoBack => {
                if let Some(wv) = self.active_webview() {
                    webview_hooks::go_back(wv);
                }
            }
            IpcCommand::GoForward => {
                if let Some(wv) = self.active_webview() {
                    webview_hooks::go_forward(wv);
                }
            }
            IpcCommand::Reload => {
                if let Some(wv) = self.active_webview() {
                    let _ = wv.reload();
                }
            }
            IpcCommand::Stop => {
                if let Some(wv) = self.active_webview() {
                    webview_hooks::stop_loading(wv);
                }
            }
            IpcCommand::GoHome => self.navigate_active_tab(INTERNAL_NEWTAB_URL),
            IpcCommand::ZoomIn => self.adjust_zoom(0.1),
            IpcCommand::ZoomOut => self.adjust_zoom(-0.1),
            IpcCommand::ZoomReset => self.set_zoom(1.0),
            IpcCommand::ToggleReaderMode => self.toggle_reader_mode(),
            IpcCommand::ToggleForceDark => self.toggle_force_dark(),
            IpcCommand::TogglePip => self.toggle_pip(),
            IpcCommand::CaptureScreenshot => self.capture_screenshot(),
            IpcCommand::CopyCleanUrl => self.copy_clean_url(),
            IpcCommand::TranslateCurrentPage => self.translate_current_page(),
            IpcCommand::PrintPage => {
                if let Some(wv) = self.active_webview() {
                    let _ = wv.print();
                }
            }
            IpcCommand::ToggleFullscreen => self.set_fullscreen(!self.is_fullscreen),
            IpcCommand::OpenDevtools => {
                if let Some(wv) = self.active_webview() {
                    wv.open_devtools();
                }
            }
            IpcCommand::OpenFind => self.open_find(),
            IpcCommand::CloseFind => self.close_find(),
            IpcCommand::FindQuery {
                query,
                case_sensitive,
                step,
            } => self.run_find(query, case_sensitive, step),
            IpcCommand::ToggleBookmarkCurrent => self.toggle_bookmark_current(),
            IpcCommand::AddBookmark { title, url } => {
                self.storage.add_bookmark(&title, &url);
                self.ui_dirty = true;
            }
            IpcCommand::RemoveBookmark { id } => {
                self.storage.remove_bookmark(&id);
                self.ui_dirty = true;
            }
            IpcCommand::ImportBookmarks { browser_id } => {
                match read_chromium_bookmarks(&browser_id) {
                    Ok(items) => {
                        let mut added = 0usize;
                        for b in items {
                            if self.storage.add_bookmark(&b.title, &b.url) {
                                added += 1;
                            }
                        }
                        if added > 0 {
                            self.show_toast(format!("Importados {added} marcadores nuevos"));
                        } else {
                            self.show_toast("Todos los marcadores ya estaban en Ferrum");
                        }
                    }
                    Err(e) => self.show_toast(e),
                }
                self.ui_dirty = true;
            }
            IpcCommand::ToggleBookmarksBar => {
                self.storage.settings.show_bookmarks_bar =
                    !self.storage.settings.show_bookmarks_bar;
                self.storage.save_settings();
                self.relayout();
                self.ui_dirty = true;
            }
            IpcCommand::RemoveHistory { id } => {
                self.storage.remove_history_entry(&id);
                self.ui_dirty = true;
            }
            IpcCommand::ClearHistory => {
                self.storage.clear_history();
                self.ui_dirty = true;
            }
            IpcCommand::PauseDownload { id } => {
                webview_hooks::pause_download(&self.download_ops, &id);
            }
            IpcCommand::ResumeDownload { id } => {
                webview_hooks::resume_download(&self.download_ops, &id);
            }
            IpcCommand::CancelDownload { id } => {
                webview_hooks::cancel_download(&self.download_ops, &id);
            }
            IpcCommand::OpenDownload { path } => open_file_in_os(&path),
            IpcCommand::RevealDownload { path } => reveal_file_in_os(&path),
            IpcCommand::RemoveDownload { id } => {
                self.storage.remove_download(&id);
                self.ui_dirty = true;
            }
            IpcCommand::ClearDownloads => {
                self.storage.clear_downloads();
                self.ui_dirty = true;
            }
            IpcCommand::AddNote {
                title,
                content,
                attach_current,
            } => {
                let (src_url, src_title) = if attach_current
                    && let Some(tab) = self.active_tab()
                    && !is_internal_url(&tab.url)
                {
                    (Some(tab.url.clone()), Some(tab.title.clone()))
                } else {
                    (None, None)
                };
                self.storage.add_note(title, content, src_url, src_title);
                self.ui_dirty = true;
            }
            IpcCommand::RemoveNote { id } => {
                self.storage.remove_note(&id);
                self.ui_dirty = true;
            }
            IpcCommand::TogglePanel { panel } => {
                if panel == PanelTab::Extensions {
                    webview_hooks::list_browser_extensions(
                        &self.chrome_webview,
                        self.proxy.clone(),
                    );
                }
                self.toggle_panel(panel);
            }
            IpcCommand::ClosePanel => {
                if self.open_panel.is_some() {
                    self.open_panel = None;
                    webview_hooks::hide_webview_smooth(&self.panel_webview);
                    self.relayout();
                    self.ui_dirty = true;
                }
            }
            IpcCommand::ResizePanelDelta { dx } => {
                if self.open_panel.is_some() && !self.is_fullscreen && !self.is_pwa_mode {
                    self.panel_width = (self.panel_width - dx).clamp(300.0, 560.0);
                    self.relayout();
                }
            }
            IpcCommand::BrowseFolder { target } => {
                let title = if target == "extension" {
                    "Seleccionar carpeta de extensión Chromium (con manifest.json)"
                } else {
                    "Seleccionar carpeta de sincronización de Ferrum"
                };
                webview_hooks::pick_native_path_async(
                    true,
                    title.to_string(),
                    None,
                    None,
                    target,
                    self.proxy.clone(),
                );
            }
            IpcCommand::BrowseBackupFile => {
                webview_hooks::pick_native_path_async(
                    false,
                    "Seleccionar archivo de respaldo cifrado (.ferrum-backup)".to_string(),
                    Some("Respaldos cifrados Ferrum (*.ferrum-backup)".to_string()),
                    Some("*.ferrum-backup".to_string()),
                    "backup_file".to_string(),
                    self.proxy.clone(),
                );
            }
            IpcCommand::ToggleShieldGlobal => {
                if let Ok(mut engine) = self.adblock.write() {
                    let next = !engine.is_enabled();
                    engine.set_enabled(next);
                    self.storage.settings.shield_enabled = next;
                }
                self.storage.save_settings();
                self.reload_active_if_web();
                self.ui_dirty = true;
            }
            IpcCommand::ToggleHttpsOnly => {
                self.storage.settings.https_only = !self.storage.settings.https_only;
                self.storage.save_settings();
                self.ui_dirty = true;
            }
            IpcCommand::ToggleShieldCosmetic => {
                if let Ok(mut engine) = self.adblock.write() {
                    let next = !engine.is_cosmetic_enabled();
                    engine.set_cosmetic_enabled(next);
                    self.storage.settings.cosmetic_filtering = next;
                }
                self.storage.save_settings();
                self.sync_page_shield_scripts_on_active_tab();
                self.ui_dirty = true;
            }
            IpcCommand::ToggleCookieBanners => {
                if let Ok(mut engine) = self.adblock.write() {
                    let next = !engine.is_cookie_banners_blocked();
                    engine.set_cookie_banners_blocked(next);
                    self.storage.settings.block_cookie_banners = next;
                }
                self.storage.save_settings();
                self.sync_page_shield_scripts_on_active_tab();
                self.ui_dirty = true;
            }
            IpcCommand::ToggleAntiFingerprint => {
                if let Ok(mut engine) = self.adblock.write() {
                    let next = !engine.is_anti_fingerprint();
                    engine.set_anti_fingerprint(next);
                    self.storage.settings.anti_fingerprint = next;
                }
                self.storage.save_settings();
                self.sync_page_shield_scripts_on_active_tab();
                self.ui_dirty = true;
            }
            IpcCommand::ToggleStripTracking => {
                self.storage.settings.strip_tracking_params =
                    !self.storage.settings.strip_tracking_params;
                self.storage.save_settings();
                self.ui_dirty = true;
            }
            IpcCommand::StartElementPicker => self.start_element_picker(),
            IpcCommand::RemoveCosmeticRule { host, selector } => {
                self.storage.remove_custom_cosmetic_rule(&host, &selector);
                self.apply_user_cosmetic_on_active_tab();
                self.show_toast("Regla cosmética eliminada");
                self.ui_dirty = true;
            }
            IpcCommand::ClearCurrentSiteCookies => {
                if let Some(tab) = self.active_tab()
                    && !is_internal_url(&tab.url)
                    && let Some(wv) = &tab.webview
                {
                    let host = extract_host(&tab.url).unwrap_or_else(|| "este sitio".into());
                    webview_hooks::clear_site_cookies(wv, &tab.url, host, self.proxy.clone());
                }
            }
            IpcCommand::UpdateFilterLists => {
                if self.updating_lists {
                    return;
                }
                self.updating_lists = true;
                self.ui_dirty = true;
                let filters_dir = self.storage.filters_dir();
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    let res = download_and_compile_filter_lists(filters_dir);
                    let _ = proxy.send_event(BrowserEvent::FilterListsUpdated(res));
                });
            }
            IpcCommand::ToggleRestoreSession => {
                self.storage.settings.restore_session = !self.storage.settings.restore_session;
                self.storage.save_settings();
                self.ui_dirty = true;
            }
            IpcCommand::ToggleLiveSearchSuggestions => {
                self.storage.settings.live_search_suggestions =
                    !self.storage.settings.live_search_suggestions;
                self.storage.save_settings();
                self.ui_dirty = true;
            }
            IpcCommand::SetAutoHibernate { minutes } => {
                self.storage.settings.auto_hibernate_minutes = minutes;
                self.storage.save_settings();
                self.ui_dirty = true;
            }
            IpcCommand::SetPermission { kind, mode } => {
                match kind.as_str() {
                    "notifications" => self.storage.settings.permissions.notifications = mode,
                    "geolocation" => self.storage.settings.permissions.geolocation = mode,
                    "camera_mic" => self.storage.settings.permissions.camera_mic = mode,
                    "sensors" => self.storage.settings.permissions.sensors = mode,
                    "clipboard" => self.storage.settings.permissions.clipboard = mode,
                    _ => {}
                }
                if let Ok(mut p) = self.permissions.write() {
                    *p = self.storage.settings.permissions.clone();
                }
                self.storage.save_settings();
                self.ui_dirty = true;
            }
            IpcCommand::ToggleSiteAllowlist { host } => {
                if let Ok(mut engine) = self.adblock.write() {
                    engine.toggle_site_allowlist(&host);
                    self.storage.settings.allowlisted_domains = engine.allowlisted_domains_sorted();
                }
                self.storage.save_settings();
                self.reload_active_if_web();
                self.ui_dirty = true;
            }
            IpcCommand::RemoveAllowlistDomain { domain } => {
                if let Ok(mut engine) = self.adblock.write() {
                    engine.remove_allowlist_domain(&domain);
                    self.storage.settings.allowlisted_domains = engine.allowlisted_domains_sorted();
                }
                self.storage.save_settings();
                self.reload_active_if_web();
                self.ui_dirty = true;
            }
            IpcCommand::AddCustomBlockDomain { domain } => {
                if let Ok(mut engine) = self.adblock.write()
                    && engine.add_custom_blocked_domain(&domain)
                {
                    self.storage.settings.custom_blocked_domains = engine.custom_blocked_sorted();
                }
                self.storage.save_settings();
                self.reload_active_if_web();
                self.ui_dirty = true;
            }
            IpcCommand::RemoveCustomBlockDomain { domain } => {
                if let Ok(mut engine) = self.adblock.write() {
                    engine.remove_custom_blocked_domain(&domain);
                    self.storage.settings.custom_blocked_domains = engine.custom_blocked_sorted();
                }
                self.storage.save_settings();
                self.reload_active_if_web();
                self.ui_dirty = true;
            }
            IpcCommand::SetSearchEngine { engine } => {
                self.storage.settings.search_engine = SearchEngine::from_id(&engine);
                self.storage.save_settings();
                self.ui_dirty = true;
            }
            IpcCommand::ClearAllBrowsingData => {
                if let Some(wv) = self.active_webview() {
                    let _ = wv.clear_all_browsing_data();
                }
                self.storage.clear_history();
                self.storage.clear_downloads();
                self.show_toast("Datos de navegación limpiados");
                self.ui_dirty = true;
            }
            IpcCommand::RegisterDefaultBrowser => match register_as_windows_browser() {
                Ok(msg) => {
                    self.windows_registered = true;
                    self.show_toast(msg);
                }
                Err(e) => self.show_toast(e),
            },
            IpcCommand::CreateWindowsShortcuts => match create_windows_shortcuts() {
                Ok(msg) => self.show_toast(msg),
                Err(e) => self.show_toast(e),
            },
            IpcCommand::VaultSetup { password } => match self.vault.setup(&password) {
                Ok(()) => {
                    self.vault_error = None;
                    self.save_pending_login_if_unlocked();
                    self.show_toast("Bóveda cifrada creada (Argon2id + AES-256-GCM)");
                }
                Err(e) => {
                    self.vault_error = Some(e);
                    self.ui_dirty = true;
                }
            },
            IpcCommand::VaultUnlock { password } => match self.vault.unlock(&password) {
                Ok(()) => {
                    self.vault_error = None;
                    self.save_pending_login_if_unlocked();
                    self.show_toast("Bóveda desbloqueada");
                }
                Err(e) => {
                    self.vault_error = Some(e);
                    self.ui_dirty = true;
                }
            },
            IpcCommand::VaultLock => {
                self.vault.lock();
                self.vault_error = None;
                self.generated_password = None;
                self.show_toast("Bóveda bloqueada");
            }
            IpcCommand::VaultAdd {
                site,
                username,
                password,
                notes,
            } => match self.vault.add_credential(site, username, password, notes) {
                Ok(()) => {
                    self.vault.touch();
                    self.vault_error = None;
                    self.generated_password = None;
                    self.show_toast("Credencial cifrada y guardada");
                }
                Err(e) => {
                    self.vault_error = Some(e);
                    self.ui_dirty = true;
                }
            },
            IpcCommand::VaultRemove { id } => match self.vault.remove_credential(&id) {
                Ok(()) => {
                    self.vault.touch();
                    self.vault_error = None;
                    self.show_toast("Credencial eliminada");
                }
                Err(e) => {
                    self.vault_error = Some(e);
                    self.ui_dirty = true;
                }
            },
            IpcCommand::VaultCopyPassword { id } => {
                self.vault.touch();
                if let Some(cred) = self.vault.find_by_id(&id) {
                    if webview_hooks::copy_secret_with_auto_clear(&cred.password, 30) {
                        self.show_toast("Contraseña copiada (se borrará del portapapeles en 30 s)");
                    } else {
                        self.show_toast("No se pudo acceder al portapapeles");
                    }
                }
            }
            IpcCommand::VaultAutofill { id } => {
                self.vault.touch();
                let creds = self
                    .vault
                    .find_by_id(&id)
                    .map(|c| (c.username.clone(), c.password.clone()));
                if let Some((user, pass)) = creds {
                    self.inject_autofill(&user, &pass);
                }
            }
            IpcCommand::QuickAutofill => self.quick_autofill(),
            IpcCommand::AcceptPendingLogin => {
                if self.vault.is_unlocked() {
                    self.save_pending_login_if_unlocked();
                    self.show_toast("Acceso guardado en tu Bóveda cifrada");
                } else {
                    self.open_panel_to(PanelTab::Vault);
                    self.show_toast("Desbloquea tu Bóveda para guardar el acceso");
                }
            }
            IpcCommand::DismissPendingLogin => {
                self.pending_login = None;
                self.relayout();
                self.ui_dirty = true;
            }
            IpcCommand::VaultGeneratePassword { length, symbols } => {
                let pwd = generate_secure_password(length, symbols);
                let _ = webview_hooks::copy_secret_with_auto_clear(&pwd, 30);
                self.generated_password = Some(pwd);
                self.show_toast("Contraseña segura generada (portapapeles se limpia en 30 s)");
            }
            IpcCommand::VaultImportCsv { csv } => match self.vault.import_csv(&csv) {
                Ok(n) => {
                    self.vault_error = None;
                    self.show_toast(if n > 0 {
                        format!("Importadas {n} contraseñas a tu Bóveda cifrada")
                    } else {
                        "No se encontraron contraseñas nuevas en el CSV".into()
                    });
                }
                Err(e) => {
                    self.vault_error = Some(e);
                    self.ui_dirty = true;
                }
            },
            IpcCommand::VaultExportCsv => match self.vault.export_csv() {
                Ok(csv) => {
                    let target = backups_dir().join(format!("ferrum_passwords_{}.csv", unix_now()));
                    if std::fs::write(&target, csv).is_ok() {
                        self.show_toast(format!(
                            "CSV exportado en Ferrum Respaldos ({})",
                            target
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("passwords.csv")
                        ));
                    } else {
                        self.show_toast("No se pudo escribir el archivo CSV");
                    }
                }
                Err(e) => self.show_toast(e),
            },
            IpcCommand::SetVaultAutoLock { minutes } => {
                self.storage.settings.vault_auto_lock_minutes = minutes;
                self.storage.save_settings();
                self.ui_dirty = true;
            }
            IpcCommand::AddChromiumExtension { folder_path } => {
                let clean = folder_path.trim().trim_matches('"');
                if !clean.is_empty() {
                    webview_hooks::add_browser_extension(
                        &self.chrome_webview,
                        clean,
                        self.proxy.clone(),
                    );
                }
            }
            IpcCommand::ToggleChromiumExtension { id, enabled } => {
                webview_hooks::toggle_browser_extension(
                    &self.chrome_webview,
                    id,
                    enabled,
                    self.proxy.clone(),
                );
            }
            IpcCommand::RemoveChromiumExtension { id } => {
                webview_hooks::remove_browser_extension(
                    &self.chrome_webview,
                    id,
                    self.proxy.clone(),
                );
            }
            IpcCommand::AddUserScript {
                name,
                match_pattern,
                code,
            } => {
                if self.userscripts.add(name, match_pattern, code) {
                    self.show_toast("Userscript guardado y activado");
                    self.ui_dirty = true;
                }
            }
            IpcCommand::ToggleUserScript { id } => {
                self.userscripts.toggle(&id);
                self.ui_dirty = true;
            }
            IpcCommand::RemoveUserScript { id } => {
                self.userscripts.remove(&id);
                self.show_toast("Userscript eliminado");
                self.ui_dirty = true;
            }
            IpcCommand::SetSyncFolder { path } => {
                let trimmed = path.trim().trim_matches('"').to_string();
                self.storage.settings.sync_folder_path = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                };
                self.storage.save_settings();
                self.show_toast("Carpeta de sincronización actualizada");
            }
            IpcCommand::ExportEncryptedBackup {
                passphrase,
                use_sync_folder,
            } => {
                let target = if use_sync_folder {
                    let Some(dir) = &self.storage.settings.sync_folder_path else {
                        self.show_toast("Configura primero una carpeta de sincronización");
                        return;
                    };
                    PathBuf::from(dir).join("ferrum-sync.ferrum-backup")
                } else {
                    backups_dir().join(format!("ferrum_{}.ferrum-backup", unix_now()))
                };
                match export_encrypted_backup(&self.storage.data_dir(), &target, &passphrase) {
                    Ok(path) => {
                        let name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("respaldo.ferrum-backup");
                        self.show_toast(format!("Respaldo cifrado guardado: {name}"));
                    }
                    Err(e) => self.show_toast(e),
                }
            }
            IpcCommand::ImportEncryptedBackup { path, passphrase } => {
                let trimmed = path.trim().trim_matches('"');
                let src = if !trimmed.is_empty() {
                    PathBuf::from(trimmed)
                } else if let Some(dir) = &self.storage.settings.sync_folder_path {
                    PathBuf::from(dir).join("ferrum-sync.ferrum-backup")
                } else {
                    self.show_toast("Indica la ruta del archivo .ferrum-backup a restaurar");
                    return;
                };
                match import_encrypted_backup(&self.storage.data_dir(), &src, &passphrase) {
                    Ok(count) => {
                        self.storage.reload_from_disk();
                        self.userscripts.reload();
                        self.vault.lock();
                        self.relayout();
                        self.show_toast(format!("Respaldo restaurado ({count} módulos)"));
                    }
                    Err(e) => self.show_toast(e),
                }
            }
            IpcCommand::CheckForUpdates => self.start_update_check(false),
            IpcCommand::InstallUpdate => self.start_update_install(),
            IpcCommand::WindowDrag => {
                let _ = self.window.drag_window();
            }
            IpcCommand::WindowMinimize => {
                self.window.set_minimized(true);
            }
            IpcCommand::WindowToggleMaximize => {
                let next = !self.window.is_maximized();
                self.window.set_maximized(next);
                self.relayout();
                self.ui_dirty = true;
            }
            IpcCommand::WindowClose => {
                let _ = self.proxy.send_event(BrowserEvent::RequestExit);
            }
        }
    }

    fn start_update_check(&mut self, silent: bool) {
        if self.update_checking || self.update_installing {
            return;
        }
        self.update_checking = true;
        if !silent {
            self.update_status = Some("Comprobando GitHub Releases...".into());
        }
        self.ui_dirty = true;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = check_for_updates();
            let _ = proxy.send_event(BrowserEvent::UpdateCheckFinished { silent, result });
        });
    }

    fn start_update_install(&mut self) {
        if self.update_installing {
            return;
        }
        let Some(info) = self.available_update.clone() else {
            self.start_update_check(false);
            return;
        };
        self.update_installing = true;
        self.update_status = Some(format!(
            "Descargando v{} y verificando firma SHA-256...",
            info.version
        ));
        self.show_toast(format!(
            "Descargando Ferrum v{} y verificando SHA-256...",
            info.version
        ));
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = download_verify_and_install(&info);
            let _ = proxy.send_event(BrowserEvent::UpdateInstallFinished(result));
        });
    }

    fn save_pending_login_if_unlocked(&mut self) {
        if !self.vault.is_unlocked() {
            return;
        }
        if let Some(p) = self.pending_login.take() {
            let _ = self
                .vault
                .add_credential(p.host, p.username, p.password, String::new());
            self.relayout();
            self.ui_dirty = true;
        }
    }

    fn quick_autofill(&mut self) {
        let Some(host) = self.active_tab().and_then(|t| extract_host(&t.url)) else {
            return;
        };
        if !self.vault.is_unlocked() {
            self.open_panel_to(PanelTab::Vault);
            self.show_toast("Desbloquea tu Bóveda para autocompletar");
            return;
        }
        self.vault.touch();
        let creds = self
            .vault
            .find_for_host(&host)
            .map(|c| (c.username.clone(), c.password.clone()));
        if let Some((user, pass)) = creds {
            self.inject_autofill(&user, &pass);
        } else {
            self.show_toast(format!("Sin credenciales guardadas para {host}"));
        }
    }

    fn inject_autofill(&self, user: &str, pass: &str) {
        if let Some(wv) = self.active_webview() {
            let u_json = serde_json::to_string(user).unwrap_or_else(|_| "\"\"".into());
            let p_json = serde_json::to_string(pass).unwrap_or_else(|_| "\"\"".into());
            let js = format!(
                "window.__ferrumAutofill ? window.__ferrumAutofill({u_json}, {p_json}) : '0'"
            );
            let proxy = self.proxy.clone();
            let _ = wv.evaluate_script_with_callback(&js, move |raw| {
                let count = raw.trim_matches('"').trim().parse::<usize>().unwrap_or(0);
                let _ = proxy.send_event(BrowserEvent::AutofillResult(count));
            });
        }
    }

    fn start_element_picker(&mut self) {
        let Some(tab) = self.active_tab() else { return };
        if is_internal_url(&tab.url) {
            self.show_toast("Abre un sitio web para usar el Selector de elementos");
            return;
        }
        if let Some(wv) = &tab.webview {
            let _ = wv.evaluate_script(
                "window.__ferrumStartElementPicker && window.__ferrumStartElementPicker();",
            );
            let _ = wv.focus();
        }
    }

    fn apply_user_cosmetic_on_active_tab(&self) {
        if let Some(tab) = self.active_tab()
            && let Some(wv) = &tab.webview
            && let Some(host) = extract_host(&tab.url)
        {
            let css = self
                .storage
                .custom_cosmetic_css_for_host(&host)
                .unwrap_or_default();
            let css_json = serde_json::to_string(&css).unwrap_or_else(|_| "\"\"".into());
            let js = format!(
                "window.__ferrumSetUserCosmetic && window.__ferrumSetUserCosmetic({css_json});"
            );
            let _ = wv.evaluate_script(&js);
        }
    }

    fn any_audio_tab(&self) -> bool {
        self.tabs.iter().any(|t| t.is_playing_audio || t.is_muted)
    }

    fn handle_omnibox_query(&mut self, query: String, left: f64, width: f64) {
        self.overlay_query = query.clone();
        self.overlay_rect = (left, width);

        if self.is_fullscreen || self.is_pwa_mode || query.trim().is_empty() {
            self.close_overlay();
            return;
        }

        let tab_summaries = self.build_tab_summaries_light();
        let suggestions = build_suggestions(
            &query,
            &tab_summaries,
            self.active_tab_id,
            &self.storage.bookmarks,
            &self.storage.history,
            self.storage.settings.search_engine,
            &[],
        );

        if suggestions.is_empty() {
            self.close_overlay();
            return;
        }

        self.overlay_suggestions = suggestions;
        self.overlay_selected = 0;
        self.overlay_visible = true;
        self.position_and_push_overlay();

        // Spawn non-blocking live DuckDuckGo autocomplete query if enabled
        if self.storage.settings.live_search_suggestions && query.trim().len() >= 2 {
            self.live_query_seq = self.live_query_seq.wrapping_add(1);
            let seq = self.live_query_seq;
            let q_clone = query;
            let proxy = self.proxy.clone();
            std::thread::spawn(move || {
                let web = fetch_web_suggestions(&q_clone);
                if !web.is_empty() {
                    let _ = proxy.send_event(BrowserEvent::WebSuggestionsReady {
                        seq,
                        query: q_clone,
                        suggestions: web,
                    });
                }
            });
        }
    }

    fn handle_omnibox_key(&mut self, key: &str) {
        match key {
            "ArrowDown" => {
                if !self.overlay_suggestions.is_empty() {
                    self.overlay_selected =
                        (self.overlay_selected + 1) % self.overlay_suggestions.len();
                    self.push_overlay_state();
                }
            }
            "ArrowUp" => {
                if !self.overlay_suggestions.is_empty() {
                    let len = self.overlay_suggestions.len();
                    self.overlay_selected = (self.overlay_selected + len - 1) % len;
                    self.push_overlay_state();
                }
            }
            "Enter" => {
                if !self.overlay_suggestions.is_empty() {
                    self.execute_suggestion(self.overlay_selected);
                } else if !self.overlay_query.trim().is_empty() {
                    let q = self.overlay_query.clone();
                    self.close_overlay();
                    let target = resolve_omnibox_input(
                        &q,
                        self.storage.settings.search_engine,
                        self.storage.settings.https_only,
                        self.storage.settings.strip_tracking_params,
                    );
                    self.navigate_active_tab(&target);
                }
            }
            _ => {}
        }
    }

    fn execute_suggestion(&mut self, index: usize) {
        let Some(item) = self.overlay_suggestions.get(index).cloned() else {
            self.close_overlay();
            return;
        };
        self.close_overlay();
        if item.kind == SuggestionKind::SwitchTab
            && let Some(tab_id) = item.tab_id
        {
            self.switch_tab(tab_id);
            return;
        }
        let target = resolve_omnibox_input(
            &item.value,
            self.storage.settings.search_engine,
            self.storage.settings.https_only,
            self.storage.settings.strip_tracking_params,
        );
        self.navigate_active_tab(&target);
    }

    fn close_overlay(&mut self) {
        if self.overlay_visible {
            self.overlay_visible = false;
            self.overlay_suggestions.clear();
            self.overlay_selected = 0;
            webview_hooks::hide_webview_smooth(&self.overlay_webview);
        }
    }

    fn position_and_push_overlay(&self) {
        let (win_w, win_h) = logical_window_size(&self.window);
        let toolbar_h = match self.storage.settings.layout_mode {
            LayoutMode::Minimal => TOOLBAR_HEIGHT_MINIMAL,
            LayoutMode::Compact => TOOLBAR_HEIGHT_COMPACT,
            LayoutMode::Vertical => TOOLBAR_HEIGHT_VERTICAL,
        };
        let top_y = toolbar_h.saturating_sub(3);
        let raw_w = (self.overlay_rect.1.round() as u32).max(320);
        let x = (self.overlay_rect.0.round().max(8.0) as u32).min(win_w.saturating_sub(280));
        let w = raw_w.min(win_w.saturating_sub(x + 8)).max(240);
        let rows = self.overlay_suggestions.len() as u32;
        let h = (rows * 38 + 12).min(win_h.saturating_sub(top_y + 20));

        webview_hooks::set_webview_bounds_sync(
            &self.overlay_webview,
            Rect {
                position: LogicalPosition::new(x, top_y).into(),
                size: LogicalSize::new(w, h).into(),
            },
            self.window.scale_factor(),
        );
        webview_hooks::bring_webview_to_top(&self.overlay_webview);
        webview_hooks::show_webview_smooth(&self.overlay_webview);
        self.push_overlay_state();
    }

    fn push_overlay_state(&self) {
        let payload = OverlayState {
            suggestions: self.overlay_suggestions.clone(),
            selected_index: self.overlay_selected,
        };
        if let Ok(json) = serde_json::to_string(&payload) {
            let js =
                format!("window.__ferrumUpdateOverlay && window.__ferrumUpdateOverlay({json});");
            let _ = self.overlay_webview.evaluate_script(&js);
        }
    }

    fn create_tab(
        &mut self,
        initial_url: &str,
        activate: bool,
        is_private: bool,
    ) -> wry::Result<u64> {
        let ws = self.active_workspace_id.clone();
        self.create_tab_full(initial_url, activate, is_private, false, ws)
    }

    fn create_hibernated_tab(
        &mut self,
        initial_url: &str,
        saved_title: &str,
        pinned: bool,
        workspace_id: String,
    ) -> u64 {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let display_url = to_display_url(initial_url);
        let initial_host = extract_host(&display_url);
        let title = if !saved_title.trim().is_empty() {
            saved_title.to_string()
        } else if is_internal_url(&display_url) {
            "Nueva pestaña".to_string()
        } else {
            initial_host.clone().unwrap_or_else(|| display_url.clone())
        };

        self.tabs.push(Tab {
            id: tab_id,
            webview: None,
            is_visible: false,
            title,
            url: display_url,
            favicon_url: None,
            host_cell: Arc::new(RwLock::new(initial_host)),
            workspace_id,
            pinned,
            is_private: false,
            loading: false,
            can_go_back: false,
            can_go_forward: false,
            is_playing_audio: false,
            is_muted: false,
            zoom_factor: 1.0,
            reader_active: false,
            force_dark_active: false,
            last_active: Instant::now(),
        });
        self.ui_dirty = true;
        tab_id
    }

    fn create_tab_full(
        &mut self,
        initial_url: &str,
        activate: bool,
        is_private: bool,
        pinned: bool,
        workspace_id: String,
    ) -> wry::Result<u64> {
        self.refresh_newtab_snapshot(is_private);

        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let display_url = to_display_url(initial_url);
        let initial_host = extract_host(&display_url);
        let bounds = self.primary_content_bounds();

        let is_isolated_ws = !is_private
            && self
                .storage
                .settings
                .workspaces
                .iter()
                .any(|w| w.id == workspace_id && w.isolated_container);

        // 0 ms Instant New Tab: claim our pre-warmed hidden Spare WebView whenever eligible!
        let (webview, host_cell, used_spare) =
            if initial_url == INTERNAL_NEWTAB_URL && !is_private && !is_isolated_ws {
                if let Some(spare) = self.spare_tab.take() {
                    spare.tab_id_cell.store(tab_id, Ordering::Relaxed);
                    (spare.webview, spare.host_cell, true)
                } else {
                    let hc = Arc::new(RwLock::new(initial_host.clone()));
                    let wv = self.build_tab_webview(
                        Arc::new(AtomicU64::new(tab_id)),
                        initial_url,
                        is_private,
                        &workspace_id,
                        hc.clone(),
                        bounds,
                    )?;
                    (wv, hc, false)
                }
            } else {
                let hc = Arc::new(RwLock::new(initial_host.clone()));
                let wv = self.build_tab_webview(
                    Arc::new(AtomicU64::new(tab_id)),
                    initial_url,
                    is_private,
                    &workspace_id,
                    hc.clone(),
                    bounds,
                )?;
                (wv, hc, false)
            };

        let initial_title = if is_internal_url(&display_url) {
            if is_private {
                "Pestaña privada".to_string()
            } else {
                "Nueva pestaña".to_string()
            }
        } else {
            initial_host.unwrap_or_else(|| display_url.clone())
        };

        self.tabs.push(Tab {
            id: tab_id,
            webview: Some(webview),
            is_visible: false,
            title: initial_title,
            url: display_url,
            favicon_url: None,
            host_cell,
            workspace_id,
            pinned,
            is_private,
            loading: !used_spare && !is_internal_url(initial_url),
            can_go_back: false,
            can_go_forward: false,
            is_playing_audio: false,
            is_muted: false,
            zoom_factor: 1.0,
            reader_active: false,
            force_dark_active: false,
            last_active: Instant::now(),
        });

        if activate {
            self.switch_tab(tab_id);
        } else {
            self.apply_tab_visibility();
        }
        // Debounce replenishing the spare tab until 1.2s after the user finishes opening tabs
        if self.spare_tab.is_none() {
            self.schedule_spare_tab_warm(Duration::from_millis(1200));
        }
        self.mark_session_dirty();
        self.ui_dirty = true;
        Ok(tab_id)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_tab_webview(
        &self,
        tab_id_cell: Arc<AtomicU64>,
        initial_url: &str,
        is_private: bool,
        workspace_id: &str,
        host_cell: Arc<RwLock<Option<String>>>,
        bounds: Rect,
    ) -> wry::Result<WebView> {
        let proxy_title = self.proxy.clone();
        let proxy_load = self.proxy.clone();
        let proxy_newwin = self.proxy.clone();
        let proxy_ipc = self.proxy.clone();
        let tid_title = tab_id_cell.clone();
        let tid_load = tab_id_cell.clone();
        let host_cell_nav = host_cell.clone();
        let newtab_snap = self.newtab_snapshot.clone();
        let perms_ref = self.permissions.clone();
        let nonce_ref = self.bridge_nonce.clone();

        // Always create WebViews hidden & unfocused so WebView2 initializes completely off-screen
        let builder = WebViewBuilder::new()
            .with_bounds(bounds)
            .with_visible(false)
            .with_focused(false)
            .with_incognito(is_private)
            .with_background_color((0, 0, 0, 255))
            .with_devtools(true)
            .with_clipboard(true)
            .with_hotkeys_zoom(true)
            .with_back_forward_navigation_gestures(true)
            .with_initialization_script(&self.page_init_script)
            .with_permission_handler(move |kind| {
                let policy = perms_ref.read().ok();
                let mode = match (&policy, kind) {
                    (Some(p), PermissionKind::Notifications) => p.notifications,
                    (Some(p), PermissionKind::Geolocation) => p.geolocation,
                    (Some(p), PermissionKind::Camera | PermissionKind::Microphone) => p.camera_mic,
                    (Some(p), PermissionKind::Sensors) => p.sensors,
                    (Some(p), PermissionKind::ClipboardRead) => p.clipboard,
                    (_, PermissionKind::Autoplay) => PermissionMode::Allow,
                    _ => PermissionMode::Ask,
                };
                match mode {
                    PermissionMode::Allow => PermissionResponse::Allow,
                    PermissionMode::Deny => PermissionResponse::Deny,
                    PermissionMode::Ask => PermissionResponse::Default,
                }
            })
            .with_custom_protocol(INTERNAL_PROTOCOL.into(), move |_id, _req| {
                let data_json = newtab_snap
                    .read()
                    .ok()
                    .map(|d| {
                        let mut cloned = d.clone();
                        cloned.is_private = is_private;
                        serde_json::to_string(&cloned).unwrap_or_else(|_| "{}".to_string())
                    })
                    .unwrap_or_else(|| "{}".to_string());
                let html = NEWTAB_HTML.replace("__FERRUM_NEWTAB_DATA__", &data_json);
                Response::builder()
                    .header("Content-Type", "text/html; charset=utf-8")
                    .header("Cache-Control", "no-store")
                    .body(Cow::Owned(html.into_bytes()))
                    .unwrap()
            })
            .with_document_title_changed_handler(move |title| {
                let tab_id = tid_title.load(Ordering::Relaxed);
                if tab_id > 0 {
                    let _ = proxy_title.send_event(BrowserEvent::TabTitleChanged { tab_id, title });
                }
            })
            .with_on_page_load_handler(move |event, url| {
                let tab_id = tid_load.load(Ordering::Relaxed);
                if tab_id > 0 {
                    let started = matches!(event, PageLoadEvent::Started);
                    let _ = proxy_load.send_event(BrowserEvent::TabLoadEvent {
                        tab_id,
                        started,
                        url,
                    });
                }
            })
            .with_navigation_handler(move |url| {
                let display = to_display_url(&url);
                if let Ok(mut cell) = host_cell_nav.write() {
                    *cell = extract_host(&display);
                }
                true
            })
            .with_new_window_req_handler(move |url, _features| {
                if !url.is_empty() && url != "about:blank" {
                    let _ = proxy_newwin.send_event(BrowserEvent::OpenNewTabUrl(url));
                }
                NewWindowResponse::Deny
            })
            .with_ipc_handler(move |req| {
                let origin = req.uri().to_string();
                if origin.starts_with(INTERNAL_ORIGIN_PREFIX) || origin.starts_with("ferrum://") {
                    if let Ok(cmd) = serde_json::from_str::<IpcCommand>(req.body()) {
                        let _ = proxy_ipc.send_event(BrowserEvent::Ipc(cmd));
                    }
                } else if let Ok(val) = serde_json::from_str::<serde_json::Value>(req.body())
                    && val.get("__ferrum_bridge_nonce").and_then(|v| v.as_str())
                        == Some(nonce_ref.as_str())
                {
                    // Authenticated nonce-guarded internal page bridge (Element Picker + Login Capture only)
                    match val.get("kind").and_then(|v| v.as_str()) {
                        Some("element_picked") => {
                            if let (Some(host), Some(selector)) = (
                                val.get("host").and_then(|v| v.as_str()),
                                val.get("selector").and_then(|v| v.as_str()),
                            ) {
                                let _ = proxy_ipc.send_event(BrowserEvent::ElementPicked {
                                    host: host.to_string(),
                                    selector: selector.to_string(),
                                });
                            }
                        }
                        Some("login_detected") => {
                            if let (Some(host), Some(password)) = (
                                val.get("host").and_then(|v| v.as_str()),
                                val.get("password").and_then(|v| v.as_str()),
                            ) {
                                let username = val
                                    .get("username")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let _ = proxy_ipc.send_event(BrowserEvent::LoginDetected {
                                    host: host.to_string(),
                                    username,
                                    password: password.to_string(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            })
            .with_url(initial_url);

        #[cfg(target_os = "windows")]
        let builder = {
            let mut b = builder
                .with_environment(self.shared_env.clone())
                .with_theme(Theme::Dark)
                .with_default_context_menus(true)
                .with_scroll_bar_style(ScrollBarStyle::FluentOverlay);

            let is_isolated_ws = !is_private
                && self
                    .storage
                    .settings
                    .workspaces
                    .iter()
                    .any(|w| w.id == workspace_id && w.isolated_container);
            if is_isolated_ws {
                b = b.with_profile_name(workspace_profile_name(workspace_id));
            }
            b
        };

        let webview = builder.build_as_child(&self.window)?;
        webview_hooks::set_webview_bounds_sync(&webview, bounds, self.window.scale_factor());
        webview_hooks::attach_content_hooks(
            &webview,
            tab_id_cell,
            host_cell,
            self.adblock.clone(),
            self.download_ops.clone(),
            self.proxy.clone(),
        );
        Ok(webview)
    }

    fn hibernate_tab(&mut self, id: u64, silent: bool) {
        if id == self.active_tab_id || self.split_tab_id == Some(id) {
            return;
        }
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else {
            return;
        };
        let Some(wv) = tab.webview.take() else {
            return;
        };
        webview_hooks::destroy_webview_smooth(wv);
        tab.is_visible = false;
        tab.loading = false;
        tab.is_playing_audio = false;
        tab.reader_active = false;
        tab.force_dark_active = false;
        if !silent {
            self.show_toast("Pestaña hibernada · RAM liberada");
        } else {
            self.ui_dirty = true;
        }
    }

    fn ensure_tab_awake(&mut self, id: u64) {
        let info = self.tabs.iter().find(|t| t.id == id).and_then(|t| {
            if t.webview.is_some() {
                return None;
            }
            let target = if is_internal_url(&t.url) {
                INTERNAL_NEWTAB_URL.to_string()
            } else {
                t.url.clone()
            };
            Some((
                target,
                t.is_private,
                t.workspace_id.clone(),
                t.host_cell.clone(),
                t.zoom_factor,
                t.is_muted,
            ))
        });

        let Some((target, is_private, ws_id, host_cell, zoom, muted)) = info else {
            return;
        };

        self.refresh_newtab_snapshot(is_private);
        let bounds = self.primary_content_bounds();
        if let Ok(wv) = self.build_tab_webview(
            Arc::new(AtomicU64::new(id)),
            &target,
            is_private,
            &ws_id,
            host_cell,
            bounds,
        ) {
            if (zoom - 1.0).abs() > 0.01 {
                webview_hooks::set_zoom_factor(&wv, zoom);
            }
            if muted {
                webview_hooks::set_muted(&wv, true);
            }
            if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                tab.webview = Some(wv);
                tab.is_visible = false;
                tab.loading = !is_internal_url(&target);
                tab.last_active = Instant::now();
            }
        }
    }

    fn reorder_tab(&mut self, from_id: u64, to_id: u64) {
        let Some(from_idx) = self.tabs.iter().position(|t| t.id == from_id) else {
            return;
        };
        let Some(to_idx) = self.tabs.iter().position(|t| t.id == to_id) else {
            return;
        };
        if from_idx == to_idx {
            return;
        }
        let item = self.tabs.remove(from_idx);
        let insert_idx = to_idx.min(self.tabs.len());
        self.tabs.insert(insert_idx, item);
        self.save_session_now();
        self.ui_dirty = true;
    }

    fn switch_tab(&mut self, id: u64) {
        let Some(ws_id) = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.workspace_id.clone())
        else {
            return;
        };

        self.close_overlay();

        let now = Instant::now();
        if let Some(prev) = self.tabs.iter_mut().find(|t| t.id == self.active_tab_id) {
            prev.last_active = now;
        }

        if self.split_tab_id == Some(id) {
            self.split_tab_id = Some(self.active_tab_id);
        }
        self.active_tab_id = id;
        self.active_workspace_id = ws_id;
        self.context_tab_id = None;

        self.ensure_tab_awake(id);
        if let Some(cur) = self.tabs.iter_mut().find(|t| t.id == id) {
            cur.last_active = now;
        }

        self.relayout();
        self.update_window_title();
        self.mark_session_dirty();
        self.ui_dirty = true;
    }

    fn close_tab(&mut self, id: u64) {
        let Some(pos) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };

        let removed = self.tabs.remove(pos);
        if !removed.is_private && !is_internal_url(&removed.url) {
            self.closed_tabs.push(ClosedTabInfo {
                title: removed.title.clone(),
                url: removed.url.clone(),
                closed_at: unix_now(),
            });
            if self.closed_tabs.len() > 25 {
                self.closed_tabs.remove(0);
            }
        }
        if let Ok(mut engine) = self.adblock.write() {
            engine.remove_tab_stats(id);
        }
        if self.split_tab_id == Some(id) {
            self.split_tab_id = None;
        }
        if self.context_tab_id == Some(id) {
            self.context_tab_id = None;
        }
        if self.audio_hub_open && !self.any_audio_tab() {
            self.audio_hub_open = false;
        }

        // 1. FIRST reveal replacement/next tab BEFORE hiding & destroying the closed WebView
        if self.tabs.is_empty() {
            let _ = self.create_tab(INTERNAL_NEWTAB_URL, true, false);
        } else if self.active_tab_id == id {
            let ws_next = self
                .tabs
                .iter()
                .find(|t| t.workspace_id == self.active_workspace_id)
                .map(|t| t.id)
                .unwrap_or_else(|| self.tabs[pos.min(self.tabs.len() - 1)].id);
            self.switch_tab(ws_next);
        } else {
            self.relayout();
            self.mark_session_dirty();
            self.ui_dirty = true;
        }

        // 2. ONLY NOW destroy the closed tab completely off-screen
        if let Some(wv) = removed.webview {
            webview_hooks::destroy_webview_smooth(wv);
        }
    }

    fn close_other_tabs(&mut self, keep_id: u64) {
        let ids_to_close: Vec<u64> = self
            .tabs
            .iter()
            .filter(|t| t.id != keep_id && !t.pinned && t.workspace_id == self.active_workspace_id)
            .map(|t| t.id)
            .collect();
        self.context_tab_id = None;
        for id in ids_to_close {
            self.close_tab(id);
        }
        self.switch_tab(keep_id);
    }

    fn duplicate_tab(&mut self, id: u64) {
        let Some((url, is_private)) = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .map(|t| (t.url.clone(), t.is_private))
        else {
            return;
        };
        let target = if is_internal_url(&url) {
            INTERNAL_NEWTAB_URL.to_string()
        } else {
            url
        };
        self.context_tab_id = None;
        let _ = self.create_tab(&target, true, is_private);
    }

    fn toggle_pin_tab(&mut self, id: u64) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.pinned = !tab.pinned;
        }
        self.context_tab_id = None;
        self.save_session_now();
        self.relayout();
        self.ui_dirty = true;
    }

    fn toggle_mute_tab(&mut self, id: u64) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.is_muted = !tab.is_muted;
            if let Some(wv) = &tab.webview {
                webview_hooks::set_muted(wv, tab.is_muted);
            }
        }
        self.context_tab_id = None;
        self.ui_dirty = true;
    }

    fn toggle_split_view(&mut self, target_tab_id: Option<u64>) {
        self.context_tab_id = None;
        if let Some(target) = target_tab_id {
            if target == self.active_tab_id {
                return;
            }
            if self.split_tab_id == Some(target) {
                self.split_tab_id = None;
            } else {
                self.ensure_tab_awake(target);
                self.split_tab_id = Some(target);
            }
        } else if self.split_tab_id.is_some() {
            self.split_tab_id = None;
        } else {
            let candidate = self
                .tabs
                .iter()
                .find(|t| t.id != self.active_tab_id && t.workspace_id == self.active_workspace_id)
                .or_else(|| self.tabs.iter().find(|t| t.id != self.active_tab_id))
                .map(|t| t.id);

            let second_id = match candidate {
                Some(id) => {
                    self.ensure_tab_awake(id);
                    id
                }
                None => match self.create_tab(INTERNAL_NEWTAB_URL, false, false) {
                    Ok(id) => id,
                    Err(_) => return,
                },
            };
            self.split_tab_id = Some(second_id);
        }
        self.relayout();
        self.ui_dirty = true;
    }

    fn switch_workspace(&mut self, workspace_id: &str) {
        if !self
            .storage
            .settings
            .workspaces
            .iter()
            .any(|w| w.id == workspace_id)
        {
            return;
        }
        self.active_workspace_id = workspace_id.to_string();
        self.split_tab_id = None;
        self.context_tab_id = None;

        if let Some(tab_id) = self
            .tabs
            .iter()
            .find(|t| t.workspace_id == self.active_workspace_id)
            .map(|t| t.id)
        {
            self.switch_tab(tab_id);
        } else {
            let _ = self.create_tab(INTERNAL_NEWTAB_URL, true, false);
        }
    }

    fn cycle_tab(&mut self, delta: isize) {
        let ws_tabs: Vec<u64> = self
            .tabs
            .iter()
            .filter(|t| t.workspace_id == self.active_workspace_id)
            .map(|t| t.id)
            .collect();
        if ws_tabs.len() <= 1 {
            return;
        }
        if let Some(pos) = ws_tabs.iter().position(|&id| id == self.active_tab_id) {
            let len = ws_tabs.len() as isize;
            let next = ((pos as isize + delta) % len + len) % len;
            self.switch_tab(ws_tabs[next as usize]);
        }
    }

    fn jump_to_workspace_tab(&mut self, idx: usize) {
        let ws_tabs: Vec<u64> = self
            .tabs
            .iter()
            .filter(|t| t.workspace_id == self.active_workspace_id)
            .map(|t| t.id)
            .collect();
        if ws_tabs.is_empty() {
            return;
        }
        let target_idx = if idx >= 8 {
            ws_tabs.len() - 1
        } else {
            idx.min(ws_tabs.len() - 1)
        };
        self.switch_tab(ws_tabs[target_idx]);
    }

    fn navigate_active_tab(&mut self, url: &str) {
        let active_id = self.active_tab_id;
        self.ensure_tab_awake(active_id);
        let is_priv = self.active_tab().map(|t| t.is_private).unwrap_or(false);
        self.refresh_newtab_snapshot(is_priv);
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == active_id) {
            let display = to_display_url(url);
            tab.url = display.clone();
            if let Ok(mut cell) = tab.host_cell.write() {
                *cell = extract_host(&display);
            }
            tab.loading = true;
            if let Some(wv) = &tab.webview {
                let _ = wv.load_url(url);
            }
        }
        self.ui_dirty = true;
    }

    fn adjust_zoom(&mut self, delta: f64) {
        let next = self
            .active_tab()
            .map(|t| ((t.zoom_factor + delta) * 10.0).round() / 10.0)
            .unwrap_or(1.0);
        self.set_zoom(next);
    }

    fn set_zoom(&mut self, factor: f64) {
        let clamped = factor.clamp(0.5, 3.0);
        let mut host_to_save = None;
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == self.active_tab_id) {
            tab.zoom_factor = clamped;
            if let Some(wv) = &tab.webview {
                webview_hooks::set_zoom_factor(wv, clamped);
            }
            host_to_save = extract_host(&tab.url);
            self.ui_dirty = true;
        }
        if let Some(host) = host_to_save {
            self.storage.set_domain_zoom(&host, clamped);
        }
    }

    fn open_find(&mut self) {
        self.close_overlay();
        self.find.open = true;
        self.relayout();
        self.ui_dirty = true;
        let _ = self.chrome_webview.focus();
        let _ = self.chrome_webview.evaluate_script(
            "setTimeout(() => window.__ferrumFocusFind && window.__ferrumFocusFind(), 20);",
        );
    }

    fn close_find(&mut self) {
        if !self.find.open {
            return;
        }
        self.find.open = false;
        self.find.query.clear();
        self.find.current = 0;
        self.find.total = 0;
        if let Some(wv) = self.active_webview() {
            let _ = wv.evaluate_script("window.__ferrumFindClear && window.__ferrumFindClear();");
        }
        self.relayout();
        self.ui_dirty = true;
    }

    fn run_find(&mut self, query: String, case_sensitive: bool, step: i32) {
        self.find.query = query.clone();
        self.find.case_sensitive = case_sensitive;
        let Some(wv) = self.active_webview() else {
            return;
        };

        let q_json = serde_json::to_string(&query).unwrap_or_else(|_| "\"\"".to_string());
        let js = format!(
            "window.__ferrumFind ? window.__ferrumFind({q_json}, {case_sensitive}, {step}) : '{{\"current\":0,\"total\":0}}'"
        );
        let proxy = self.proxy.clone();
        let _ = wv.evaluate_script_with_callback(&js, move |raw| {
            let parsed = parse_js_json_object(&raw);
            let current = parsed
                .as_ref()
                .and_then(|v| v.get("current"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let total = parsed
                .as_ref()
                .and_then(|v| v.get("total"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let _ = proxy.send_event(BrowserEvent::FindResult { current, total });
        });
    }

    fn toggle_reader_mode(&self) {
        let Some(tab) = self.active_tab() else { return };
        if is_internal_url(&tab.url) {
            return;
        }
        let Some(wv) = &tab.webview else { return };
        let tab_id = tab.id;
        let proxy = self.proxy.clone();
        let _ = wv.evaluate_script_with_callback(
            "window.__ferrumToggleReader ? window.__ferrumToggleReader() : 'false'",
            move |raw| {
                let active = raw.contains("true");
                let _ = proxy.send_event(BrowserEvent::ReaderToggled { tab_id, active });
            },
        );
    }

    fn toggle_force_dark(&self) {
        let Some(tab) = self.active_tab() else { return };
        if is_internal_url(&tab.url) {
            return;
        }
        let Some(wv) = &tab.webview else { return };
        let tab_id = tab.id;
        let proxy = self.proxy.clone();
        let _ = wv.evaluate_script_with_callback(
            "window.__ferrumToggleForceDark ? window.__ferrumToggleForceDark() : 'false'",
            move |raw| {
                let active = raw.contains("true");
                let _ = proxy.send_event(BrowserEvent::ForceDarkToggled { tab_id, active });
            },
        );
    }

    fn toggle_pip(&self) {
        let Some(tab) = self.active_tab() else { return };
        if is_internal_url(&tab.url) {
            return;
        }
        let Some(wv) = &tab.webview else { return };
        let proxy = self.proxy.clone();
        let _ = wv.evaluate_script_with_callback(
            "window.__ferrumTogglePiP ? window.__ferrumTogglePiP() : 'no_video'",
            move |raw| {
                let cleaned = raw.trim_matches('"').to_string();
                let _ = proxy.send_event(BrowserEvent::PipResult(cleaned));
            },
        );
    }

    fn capture_screenshot(&self) {
        let Some(tab) = self.active_tab() else { return };
        let Some(wv) = &tab.webview else { return };
        let host = extract_host(&tab.url)
            .unwrap_or_else(|| "ferrum".to_string())
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .take(28)
            .collect::<String>();
        let filename = format!("ferrum_{host}_{}.png", unix_now());
        let target = BrowserStorage::screenshots_dir().join(filename);
        webview_hooks::capture_screenshot_png(wv, target, self.proxy.clone());
    }

    fn copy_clean_url(&mut self) {
        let Some(url) = self.active_tab().map(|t| t.url.clone()) else {
            return;
        };
        if is_internal_url(&url) {
            self.show_toast("Abre un sitio web para copiar su enlace");
            return;
        }
        let clean = strip_tracking_params(&url);
        if webview_hooks::copy_text_to_clipboard(&clean) {
            self.show_toast("URL limpia copiada al portapapeles");
        } else {
            self.show_toast("No se pudo copiar al portapapeles");
        }
    }

    fn translate_current_page(&mut self) {
        let Some((url, is_private)) = self.active_tab().map(|t| (t.url.clone(), t.is_private))
        else {
            return;
        };
        if is_internal_url(&url) {
            self.show_toast("Abre una página web para traducirla");
            return;
        }
        let encoded = percent_encode_query(&url);
        let translate_url =
            format!("https://translate.google.com/translate?sl=auto&tl=es&u={encoded}");
        let _ = self.create_tab(&translate_url, true, is_private);
        self.show_toast("Traduciendo página al español...");
    }

    fn set_fullscreen(&mut self, fs: bool) {
        if self.is_fullscreen == fs {
            return;
        }
        self.close_overlay();
        self.is_fullscreen = fs;
        if fs {
            self.window
                .set_fullscreen(Some(Fullscreen::Borderless(None)));
            webview_hooks::hide_webview_smooth(&self.chrome_webview);
            webview_hooks::hide_webview_smooth(&self.vertical_webview);
            webview_hooks::hide_webview_smooth(&self.divider_webview);
            webview_hooks::hide_webview_smooth(&self.panel_webview);
        } else {
            self.window.set_fullscreen(None);
        }
        self.relayout();
        self.ui_dirty = true;
    }

    fn reload_active_if_web(&self) {
        if let Some(tab) = self.active_tab()
            && !is_internal_url(&tab.url)
            && let Some(wv) = &tab.webview
        {
            let _ = wv.reload();
        }
    }

    fn sync_page_shield_scripts_on_active_tab(&self) {
        if let Some(tab) = self.active_tab() {
            if is_internal_url(&tab.url) {
                return;
            }
            let Some(wv) = &tab.webview else { return };
            let host = extract_host(&tab.url);
            let (cosmetic_on, cookie_on, afp_on) = self
                .adblock
                .read()
                .map(|e| {
                    let allowed = host
                        .as_deref()
                        .map(|h| !e.is_site_allowlisted(h))
                        .unwrap_or(true);
                    (
                        e.is_enabled() && e.is_cosmetic_enabled() && allowed,
                        e.is_enabled() && e.is_cookie_banners_blocked() && allowed,
                        e.is_enabled() && e.is_anti_fingerprint() && allowed,
                    )
                })
                .unwrap_or((false, false, false));
            let js = format!(
                "window.__ferrumSetCosmetic && window.__ferrumSetCosmetic({cosmetic_on});\
                 window.__ferrumSetCookieBlock && window.__ferrumSetCookieBlock({cookie_on});\
                 window.__ferrumSetAntiFp && window.__ferrumSetAntiFp({afp_on});"
            );
            let _ = wv.evaluate_script(&js);
        }
    }

    fn open_panel_to(&mut self, panel: PanelTab) {
        if self.is_pwa_mode {
            return;
        }
        self.close_overlay();
        self.open_panel = Some(panel);
        self.relayout();
        self.ui_dirty = true;
    }

    fn toggle_panel(&mut self, panel: PanelTab) {
        if self.is_pwa_mode {
            return;
        }
        self.close_overlay();
        if self.open_panel == Some(panel) {
            self.open_panel = None;
            webview_hooks::hide_webview_smooth(&self.panel_webview);
        } else {
            self.open_panel = Some(panel);
        }
        self.relayout();
        self.ui_dirty = true;
    }

    fn toggle_bookmark_current(&mut self) {
        if let Some(tab) = self.active_tab() {
            let title = tab.title.clone();
            let url = tab.url.clone();
            let added = self.storage.toggle_bookmark(&title, &url);
            if !is_internal_url(&url) {
                self.show_toast(if added {
                    "Añadido a marcadores"
                } else {
                    "Quitado de marcadores"
                });
            }
            self.ui_dirty = true;
        }
    }

    fn focus_omnibox(&self) {
        if self.is_pwa_mode {
            return;
        }
        let _ = self.chrome_webview.focus();
        let _ = self
            .chrome_webview
            .evaluate_script("window.__ferrumFocusOmnibox && window.__ferrumFocusOmnibox();");
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == self.active_tab_id)
    }

    fn active_webview(&self) -> Option<&WebView> {
        self.active_tab().and_then(|t| t.webview.as_ref())
    }

    fn current_chrome_height(&self) -> u32 {
        if self.is_fullscreen {
            return 0;
        }
        if self.is_pwa_mode {
            return PWA_BAR_HEIGHT;
        }
        let mut h = match self.storage.settings.layout_mode {
            LayoutMode::Minimal => TOOLBAR_HEIGHT_MINIMAL + TAB_BAR_HEIGHT,
            LayoutMode::Compact => TOOLBAR_HEIGHT_COMPACT,
            LayoutMode::Vertical => TOOLBAR_HEIGHT_VERTICAL,
        };
        if self.context_tab_id.is_some() {
            h += CONTEXT_BAR_HEIGHT;
        }
        if self.audio_hub_open && self.any_audio_tab() {
            h += AUDIO_BAR_HEIGHT;
        }
        if self.pending_login.is_some() {
            h += LOGIN_BAR_HEIGHT;
        }
        if self.find.open {
            h += FIND_BAR_HEIGHT;
        }
        if self.storage.settings.show_bookmarks_bar {
            h += BOOKMARKS_BAR_HEIGHT;
        }
        h
    }

    fn effective_vertical_collapsed(&self, win_w: u32) -> bool {
        self.storage.settings.vertical_collapsed || win_w < 860
    }

    fn current_left_sidebar_width(&self, win_w: u32) -> u32 {
        if self.is_fullscreen
            || self.is_pwa_mode
            || self.storage.settings.layout_mode != LayoutMode::Vertical
        {
            return 0;
        }
        if self.effective_vertical_collapsed(win_w) {
            VERTICAL_WIDTH_COLLAPSED
        } else {
            VERTICAL_WIDTH_EXPANDED
        }
    }

    fn current_panel_width(&self, win_w: u32, left_w: u32) -> u32 {
        if !self.is_fullscreen && !self.is_pwa_mode && self.open_panel.is_some() {
            let remaining = win_w.saturating_sub(left_w);
            if remaining <= 220 {
                return remaining;
            }
            // Guarantee left sidebar + right panel NEVER overlap and content keeps >= 140px
            let max_w = remaining.saturating_sub(140).min(remaining * 58 / 100);
            (self.panel_width.round() as u32).min(max_w.max(220.min(remaining)))
        } else {
            0
        }
    }

    fn content_area_width(&self) -> u32 {
        let (win_w, _) = logical_window_size(&self.window);
        let left_w = self.current_left_sidebar_width(win_w);
        let panel_w = self.current_panel_width(win_w, left_w);
        win_w.saturating_sub(left_w + panel_w)
    }

    /// Computes exact physical-pixel rectangles (x, y, w, h) for primary content,
    /// optional split divider, and optional secondary content with zero DPI rounding gaps.
    #[allow(clippy::type_complexity)]
    fn split_layout_physical(
        &self,
        phys_w: i32,
        phys_h: i32,
        scale: f64,
    ) -> (
        (i32, i32, i32, i32),
        Option<(i32, i32, i32, i32)>,
        Option<(i32, i32, i32, i32)>,
    ) {
        let win_w_log = ((phys_w.max(1) as f64) / scale).round() as u32;
        let top_px = ((self.current_chrome_height() as f64) * scale)
            .round()
            .clamp(0.0, phys_h as f64) as i32;
        let left_log = self.current_left_sidebar_width(win_w_log);
        let left_px = ((left_log as f64) * scale)
            .round()
            .clamp(0.0, (phys_w / 3).max(0) as f64) as i32;
        let panel_log = self.current_panel_width(win_w_log, left_log);
        let panel_px = if panel_log == 0 {
            0
        } else {
            ((panel_log as f64) * scale)
                .round()
                .clamp(0.0, (phys_w - left_px).max(0) as f64) as i32
        };

        let avail_w = (phys_w - left_px - panel_px).max(0);
        let avail_h = (phys_h - top_px).max(0);
        let min_split_px = (340.0 * scale).round() as i32;

        if self.split_tab_id.is_some()
            && !self.is_fullscreen
            && !self.is_pwa_mode
            && avail_w > min_split_px
        {
            let div_px = ((DIVIDER_WIDTH as f64) * scale).round().max(4.0) as i32;
            let usable = (avail_w - div_px).max(0);
            let min_pane = ((160.0 * scale).round() as i32).min(usable / 3);
            let primary_w = ((usable as f64) * self.split_ratio)
                .round()
                .clamp(min_pane as f64, (usable - min_pane).max(0) as f64)
                as i32;
            let div_x = left_px + primary_w;
            let sec_x = div_x + div_px;
            let sec_w = (usable - primary_w).max(0);
            (
                (left_px, top_px, primary_w, avail_h),
                Some((div_x, top_px, div_px, avail_h)),
                Some((sec_x, top_px, sec_w, avail_h)),
            )
        } else {
            ((left_px, top_px, avail_w, avail_h), None, None)
        }
    }

    fn primary_content_bounds(&self) -> Rect {
        let phys = self.window.inner_size();
        let scale = self.window.scale_factor();
        let (pw, ph) = if phys.width > 0 && phys.height > 0 {
            (phys.width as i32, phys.height as i32)
        } else {
            ((1380.0 * scale) as i32, (880.0 * scale) as i32)
        };
        let ((x, y, w, h), _, _) = self.split_layout_physical(pw, ph, scale);
        Rect {
            position: wry::dpi::PhysicalPosition::new(x, y).into(),
            size: wry::dpi::PhysicalSize::new(w.max(0) as u32, h.max(0) as u32).into(),
        }
    }

    fn apply_tab_visibility_physical(&mut self, phys_w: i32, phys_h: i32, scale: f64) {
        let (primary_rect, divider_rect, secondary_rect) =
            self.split_layout_physical(phys_w, phys_h, scale);
        let active_id = self.active_tab_id;
        let split_id = self.split_tab_id;
        let is_fs = self.is_fullscreen;

        // PASS 1: Position, raise, and reveal ACTIVE tab(s) & split divider FIRST
        for tab in &mut self.tabs {
            let Some(wv) = &tab.webview else {
                tab.is_visible = false;
                continue;
            };
            let is_primary = tab.id == active_id;
            let is_secondary = !is_fs && split_id == Some(tab.id) && secondary_rect.is_some();

            if is_primary {
                let (x, y, w, h) = primary_rect;
                webview_hooks::set_webview_bounds_physical(wv, x, y, w, h);
                webview_hooks::bring_webview_to_top(wv);
                webview_hooks::show_webview_smooth(wv);
                #[cfg(target_os = "windows")]
                let _ = wv.set_memory_usage_level(MemoryUsageLevel::Normal);
                tab.is_visible = true;
            } else if let (true, Some((x, y, w, h))) = (is_secondary, secondary_rect) {
                webview_hooks::set_webview_bounds_physical(wv, x, y, w, h);
                webview_hooks::bring_webview_to_top(wv);
                webview_hooks::show_webview_smooth(wv);
                #[cfg(target_os = "windows")]
                let _ = wv.set_memory_usage_level(MemoryUsageLevel::Normal);
                tab.is_visible = true;
            }
        }

        if let Some((x, y, w, h)) = divider_rect {
            webview_hooks::set_webview_bounds_physical(&self.divider_webview, x, y, w, h);
            webview_hooks::bring_webview_to_top(&self.divider_webview);
            webview_hooks::show_webview_smooth(&self.divider_webview);
        }

        // PASS 2: Only AFTER the active view covers the viewport, hide previously visible inactive tabs
        if divider_rect.is_none() {
            webview_hooks::hide_webview_smooth(&self.divider_webview);
        }

        for tab in &mut self.tabs {
            let Some(wv) = &tab.webview else {
                tab.is_visible = false;
                continue;
            };
            let is_primary = tab.id == active_id;
            let is_secondary = !is_fs && split_id == Some(tab.id) && secondary_rect.is_some();

            if !is_primary && !is_secondary && tab.is_visible {
                webview_hooks::hide_webview_smooth(wv);
                #[cfg(target_os = "windows")]
                let _ = wv.set_memory_usage_level(MemoryUsageLevel::Low);
                tab.is_visible = false;
            }
        }
    }

    fn apply_tab_visibility(&mut self) {
        if self.window.is_minimized() {
            return;
        }
        let phys = self.window.inner_size();
        if phys.width == 0 || phys.height == 0 {
            return;
        }
        let scale = self.window.scale_factor();
        self.apply_tab_visibility_physical(phys.width as i32, phys.height as i32, scale);
    }

    fn relayout(&mut self) {
        self.relayout_with_physical(None);
    }

    fn relayout_with_physical(&mut self, phys_override: Option<(u32, u32)>) {
        if self.window.is_minimized() {
            return;
        }
        let (raw_w, raw_h) = phys_override.unwrap_or_else(|| {
            let sz = self.window.inner_size();
            (sz.width, sz.height)
        });
        // Never relayout child WebViews to 0x0 during Windows minimize/restore transitions
        if raw_w == 0 || raw_h == 0 {
            return;
        }
        self.last_layout_phys = (raw_w, raw_h);

        let phys_w = raw_w as i32;
        let phys_h = raw_h as i32;
        let scale = self.window.scale_factor();
        let win_w_log = ((raw_w as f64) / scale).round() as u32;

        let top_px = ((self.current_chrome_height() as f64) * scale)
            .round()
            .clamp(0.0, phys_h as f64) as i32;
        let left_log = self.current_left_sidebar_width(win_w_log);
        let left_px = ((left_log as f64) * scale)
            .round()
            .clamp(0.0, (phys_w / 3).max(0) as f64) as i32;
        let panel_log = self.current_panel_width(win_w_log, left_log);
        let panel_px = if panel_log == 0 {
            0
        } else {
            ((panel_log as f64) * scale)
                .round()
                .clamp(0.0, (phys_w - left_px).max(0) as f64) as i32
        };
        let body_h_px = (phys_h - top_px).max(0);

        if !self.is_fullscreen {
            webview_hooks::set_webview_bounds_physical(&self.chrome_webview, 0, 0, phys_w, top_px);
            webview_hooks::show_webview_smooth(&self.chrome_webview);

            if left_px > 0 {
                webview_hooks::set_webview_bounds_physical(
                    &self.vertical_webview,
                    0,
                    top_px,
                    left_px,
                    body_h_px,
                );
                webview_hooks::show_webview_smooth(&self.vertical_webview);
            } else {
                webview_hooks::hide_webview_smooth(&self.vertical_webview);
            }

            if panel_px > 0 {
                webview_hooks::set_webview_bounds_physical(
                    &self.panel_webview,
                    phys_w - panel_px,
                    top_px,
                    panel_px,
                    body_h_px,
                );
                webview_hooks::show_webview_smooth(&self.panel_webview);
            } else {
                webview_hooks::hide_webview_smooth(&self.panel_webview);
            }
        }

        self.apply_tab_visibility_physical(phys_w, phys_h, scale);

        if self.overlay_visible && !self.is_fullscreen {
            self.position_and_push_overlay();
        }
    }

    fn update_window_title(&self) {
        if let Some(tab) = self.active_tab() {
            if self.is_pwa_mode {
                let title = self.pwa_title.as_deref().unwrap_or(tab.title.as_str());
                self.window.set_title(title);
                return;
            }
            let prefix = if tab.is_private { "[Privado] " } else { "" };
            if tab.title.is_empty()
                || tab.title == "Nueva pestaña"
                || tab.title == "Pestaña privada"
            {
                self.window.set_title(&format!("{prefix}Ferrum Browser"));
            } else {
                self.window
                    .set_title(&format!("{prefix}{} — Ferrum", tab.title));
            }
        }
    }

    fn save_session_now(&self) {
        if self.is_pwa_mode || self.ephemeral_test || !self.storage.settings.restore_session {
            return;
        }
        let mut saved_tabs = Vec::new();
        let mut active_index = 0;
        for tab in &self.tabs {
            if tab.is_private {
                continue;
            }
            if tab.id == self.active_tab_id {
                active_index = saved_tabs.len();
            }
            saved_tabs.push(SavedTab {
                url: tab.url.clone(),
                title: tab.title.clone(),
                pinned: tab.pinned,
                workspace_id: tab.workspace_id.clone(),
            });
        }
        if !saved_tabs.is_empty() {
            self.storage.save_session(&SessionData {
                tabs: saved_tabs,
                active_index,
                active_workspace_id: self.active_workspace_id.clone(),
            });
        }
    }

    fn refresh_newtab_snapshot(&self, is_private: bool) {
        let (total_blocked, bytes_saved, rules_count, shield_enabled) = self
            .adblock
            .read()
            .map(|e| {
                (
                    e.total_blocked(),
                    e.bytes_saved(),
                    e.rules_count(),
                    e.is_enabled(),
                )
            })
            .unwrap_or((0, 0, 0, true));

        let ws_name = self
            .storage
            .settings
            .workspaces
            .iter()
            .find(|w| w.id == self.active_workspace_id)
            .map(|w| w.name.clone())
            .unwrap_or_else(|| "General".to_string());

        if let Ok(mut snap) = self.newtab_snapshot.write() {
            snap.search_engine = self.storage.settings.search_engine;
            snap.total_blocked = total_blocked;
            snap.bytes_saved = bytes_saved;
            snap.rules_count = rules_count;
            snap.shield_enabled = shield_enabled;
            snap.https_only = self.storage.settings.https_only;
            snap.anti_fingerprint = self.storage.settings.anti_fingerprint;
            snap.is_private = is_private;
            snap.workspace_name = ws_name;
            snap.bookmarks = self.storage.bookmarks.clone();
            snap.recent_history = self.storage.history.iter().take(8).cloned().collect();
            snap.closed_tabs = self.closed_tabs.iter().rev().take(6).cloned().collect();
        }
    }

    fn persist_blocked_counter_if_needed(&mut self, force: bool) {
        if self.ephemeral_test {
            return;
        }
        let (current, bytes) = self
            .adblock
            .read()
            .map(|e| (e.total_blocked(), e.bytes_saved()))
            .unwrap_or((
                self.last_persisted_blocked,
                self.storage.settings.bytes_saved_lifetime,
            ));
        if (force && current != self.last_persisted_blocked)
            || current >= self.last_persisted_blocked + 10
        {
            self.last_persisted_blocked = current;
            self.storage.settings.total_blocked_lifetime = current;
            self.storage.settings.bytes_saved_lifetime = bytes;
            self.storage.save_settings();
        }
    }

    fn build_tab_summaries_light(&self) -> Vec<TabSummary> {
        self.tabs
            .iter()
            .map(|tab| {
                let internal = is_internal_url(&tab.url);
                TabSummary {
                    id: tab.id,
                    title: tab.title.clone(),
                    url: tab.url.clone(),
                    display_url: if internal {
                        INTERNAL_NEWTAB_DISPLAY.to_string()
                    } else {
                        tab.url.clone()
                    },
                    host: extract_host(&tab.url),
                    favicon_url: None,
                    workspace_id: tab.workspace_id.clone(),
                    pinned: tab.pinned,
                    is_private: tab.is_private,
                    is_hibernated: tab.webview.is_none(),
                    loading: tab.loading,
                    can_go_back: tab.can_go_back,
                    can_go_forward: tab.can_go_forward,
                    is_playing_audio: tab.is_playing_audio,
                    is_muted: tab.is_muted,
                    zoom_percent: (tab.zoom_factor * 100.0).round() as u32,
                    reader_active: tab.reader_active,
                    force_dark_active: tab.force_dark_active,
                    blocked_count: 0,
                    is_bookmarked: false,
                    is_internal: internal,
                }
            })
            .collect()
    }

    fn build_ui_state(&self, toast: Option<String>) -> UiState {
        let active_host = self.active_tab().and_then(|t| extract_host(&t.url));
        let custom_cosmetic_rules = active_host
            .as_deref()
            .map(|h| self.storage.custom_cosmetic_selectors_for_host(h))
            .unwrap_or_default();

        let (shield, tab_counts): (ShieldSnapshot, Vec<u64>) = match self.adblock.read() {
            Ok(engine) => {
                let counts: Vec<u64> = self
                    .tabs
                    .iter()
                    .map(|t| engine.tab_blocked_count(t.id))
                    .collect();
                let site_allowlisted = active_host
                    .as_deref()
                    .map(|h| engine.is_site_allowlisted(h))
                    .unwrap_or(false);
                let snap = ShieldSnapshot {
                    enabled: engine.is_enabled(),
                    https_only: self.storage.settings.https_only,
                    cosmetic_enabled: engine.is_cosmetic_enabled(),
                    block_cookie_banners: engine.is_cookie_banners_blocked(),
                    anti_fingerprint: engine.is_anti_fingerprint(),
                    strip_tracking_params: self.storage.settings.strip_tracking_params,
                    has_community_lists: engine.has_community_lists(),
                    updating_lists: self.updating_lists,
                    lists_updated_at: self.storage.settings.filter_lists_updated_at,
                    current_host: active_host.clone(),
                    site_allowlisted,
                    tab_blocked: engine.tab_blocked_count(self.active_tab_id),
                    total_blocked: engine.total_blocked(),
                    bytes_saved: engine.bytes_saved(),
                    rules_count: engine.rules_count(),
                    recent_blocked: engine.tab_recent_events(self.active_tab_id),
                    allowlisted_domains: engine.allowlisted_domains_sorted(),
                    custom_blocked_domains: engine.custom_blocked_sorted(),
                    custom_cosmetic_rules,
                    permissions: self.storage.settings.permissions.clone(),
                };
                (snap, counts)
            }
            Err(_) => (
                ShieldSnapshot {
                    enabled: true,
                    https_only: self.storage.settings.https_only,
                    cosmetic_enabled: true,
                    block_cookie_banners: true,
                    anti_fingerprint: true,
                    strip_tracking_params: true,
                    has_community_lists: false,
                    updating_lists: self.updating_lists,
                    lists_updated_at: self.storage.settings.filter_lists_updated_at,
                    current_host: active_host.clone(),
                    site_allowlisted: false,
                    tab_blocked: 0,
                    total_blocked: 0,
                    bytes_saved: 0,
                    rules_count: 0,
                    recent_blocked: Vec::new(),
                    allowlisted_domains: Vec::new(),
                    custom_blocked_domains: Vec::new(),
                    custom_cosmetic_rules,
                    permissions: self.storage.settings.permissions.clone(),
                },
                vec![0; self.tabs.len()],
            ),
        };

        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let internal = is_internal_url(&tab.url);
                TabSummary {
                    id: tab.id,
                    title: tab.title.clone(),
                    url: tab.url.clone(),
                    display_url: if internal {
                        INTERNAL_NEWTAB_DISPLAY.to_string()
                    } else {
                        tab.url.clone()
                    },
                    host: extract_host(&tab.url),
                    favicon_url: if internal {
                        None
                    } else {
                        tab.favicon_url.clone()
                    },
                    workspace_id: tab.workspace_id.clone(),
                    pinned: tab.pinned,
                    is_private: tab.is_private,
                    is_hibernated: tab.webview.is_none(),
                    loading: tab.loading,
                    can_go_back: tab.can_go_back,
                    can_go_forward: tab.can_go_forward,
                    is_playing_audio: tab.is_playing_audio,
                    is_muted: tab.is_muted,
                    zoom_percent: (tab.zoom_factor * 100.0).round() as u32,
                    reader_active: tab.reader_active,
                    force_dark_active: tab.force_dark_active,
                    blocked_count: tab_counts[i],
                    is_bookmarked: !internal && self.storage.is_bookmarked(&tab.url),
                    is_internal: internal,
                }
            })
            .collect();

        let active_downloads_count = self
            .storage
            .downloads
            .iter()
            .filter(|d| {
                matches!(
                    d.status,
                    DownloadStatus::InProgress | DownloadStatus::Paused
                )
            })
            .count();

        let has_match_for_active = active_host
            .as_deref()
            .map(|h| self.vault.has_credentials_for_host(h))
            .unwrap_or(false);

        let vault = VaultSnapshot {
            exists: self.vault.exists(),
            unlocked: self.vault.is_unlocked(),
            error: self.vault_error.clone(),
            generated_password: self.generated_password.clone(),
            has_match_for_active,
            pending_login: self.pending_login.as_ref().map(|p| PendingLoginPrompt {
                host: p.host.clone(),
                username: p.username.clone(),
            }),
            auto_lock_minutes: self.storage.settings.vault_auto_lock_minutes,
            entries: self
                .vault
                .entries()
                .iter()
                .map(|c| VaultEntrySummary {
                    id: c.id.clone(),
                    site: c.site.clone(),
                    username: c.username.clone(),
                    notes: c.notes.clone(),
                    updated_at: c.updated_at,
                })
                .collect(),
        };

        let mut closed_tabs = self.closed_tabs.clone();
        closed_tabs.reverse();

        UiState {
            is_pwa_mode: self.is_pwa_mode,
            pwa_title: self.pwa_title.clone(),
            tabs,
            active_tab_id: self.active_tab_id,
            split_tab_id: self.split_tab_id,
            split_ratio: self.split_ratio,
            context_tab_id: self.context_tab_id,
            audio_hub_open: self.audio_hub_open,
            active_workspace_id: self.active_workspace_id.clone(),
            workspaces: self.storage.settings.workspaces.clone(),
            layout_mode: self.storage.settings.layout_mode,
            vertical_collapsed: self
                .effective_vertical_collapsed(logical_window_size(&self.window).0),
            show_bookmarks_bar: self.storage.settings.show_bookmarks_bar,
            restore_session: self.storage.settings.restore_session,
            live_search_suggestions: self.storage.settings.live_search_suggestions,
            auto_hibernate_minutes: self.storage.settings.auto_hibernate_minutes,
            windows_registered: self.windows_registered,
            sync_folder_path: self.storage.settings.sync_folder_path.clone(),
            picked_backup_path: self.picked_backup_path.clone(),
            is_fullscreen: self.is_fullscreen,
            is_maximized: self.window.is_maximized(),
            open_panel: self.open_panel,
            find: self.find.clone(),
            search_engine: self.storage.settings.search_engine,
            bookmarks: self.storage.bookmarks.clone(),
            detected_browsers: self.detected_browsers.clone(),
            history: self.storage.history.iter().take(200).cloned().collect(),
            closed_tabs,
            downloads: self.storage.downloads.iter().take(60).cloned().collect(),
            active_downloads_count,
            notes: self.storage.notes.clone(),
            extensions: self.extensions.clone(),
            userscripts: self.userscripts.scripts.clone(),
            shield,
            vault,
            updater: UpdateSnapshot {
                current_version: CURRENT_VERSION.to_string(),
                repo_url: RELEASES_PAGE_URL.to_string(),
                checking: self.update_checking,
                installing: self.update_installing,
                available: self.available_update.clone(),
                status_text: self.update_status.clone(),
            },
            toast,
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
