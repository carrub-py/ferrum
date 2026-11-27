use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tao::event_loop::EventLoopProxy;
use wry::WebView;

use crate::adblock::AdBlockEngine;
use crate::browser::{BrowserEvent, ContextMenuAction, ShortcutAction};
use crate::extensions::ChromiumExtensionInfo;
use crate::navigation::extract_host;
use crate::storage::{BrowserStorage, DownloadStatus, generate_id};

#[derive(Debug, Clone, Copy, Default)]
pub struct NavState {
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

#[cfg(target_os = "windows")]
pub type DownloadOpsMap = Rc<
    RefCell<
        HashMap<
            String,
            webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DownloadOperation,
        >,
    >,
>;

#[cfg(not(target_os = "windows"))]
pub type DownloadOpsMap = Rc<RefCell<HashMap<String, ()>>>;

#[cfg(target_os = "windows")]
pub fn attach_content_hooks(
    webview: &WebView,
    tab_id_cell: Arc<AtomicU64>,
    tab_host: Arc<RwLock<Option<String>>>,
    adblock: Arc<RwLock<AdBlockEngine>>,
    download_ops: DownloadOpsMap,
    proxy: EventLoopProxy<BrowserEvent>,
) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR,
        COREWEBVIEW2_DOWNLOAD_STATE, COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED,
        COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS, COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST,
        COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL, ICoreWebView2_4, ICoreWebView2_8,
        ICoreWebView2_11, ICoreWebView2_15, ICoreWebView2_22, ICoreWebView2Environment9,
    };
    use webview2_com::{
        BytesReceivedChangedEventHandler, ContainsFullScreenElementChangedEventHandler,
        ContextMenuRequestedEventHandler, CustomItemSelectedEventHandler,
        DownloadStartingEventHandler, FaviconChangedEventHandler, HistoryChangedEventHandler,
        IsDocumentPlayingAudioChangedEventHandler, IsMutedChangedEventHandler,
        SourceChangedEventHandler, StateChangedEventHandler, WebResourceRequestedEventHandler,
        take_pwstr,
    };
    use windows_core::{BOOL, HSTRING, Interface, PWSTR, w};
    use wry::WebViewExtWindows;

    enable_clip_siblings(webview);

    let core = webview.webview();
    let env = webview.environment();

    unsafe {
        let mut token: i64 = 0;

        // 1. Real Chromium network-level ad, tracker, EasyList & malware interception
        let star = HSTRING::from("*");
        match core.cast::<ICoreWebView2_22>() {
            Ok(core22) => {
                let _ = core22.AddWebResourceRequestedFilterWithRequestSourceKinds(
                    &star,
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                );
            }
            Err(_) => {
                let _ = core
                    .AddWebResourceRequestedFilter(&star, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL);
            }
        }

        let env_clone = env.clone();
        let tab_id_req = tab_id_cell.clone();
        let tab_host_req = tab_host.clone();
        let adblock_req = adblock.clone();
        let proxy_req = proxy.clone();

        let _ = core.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let request = args.Request()?;
                let uri = {
                    let mut raw = PWSTR::null();
                    request.Uri(&mut raw)?;
                    take_pwstr(raw)
                };

                let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
                let _ = args.ResourceContext(&mut context);
                let is_document = context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT;
                let res_type = match context {
                    c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => "document",
                    c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => "script",
                    c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => "image",
                    c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => "stylesheet",
                    c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => "font",
                    c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => "media",
                    c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST => {
                        "xmlhttprequest"
                    }
                    c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH => "fetch",
                    c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING => "ping",
                    _ => "other",
                };

                let page_host = tab_host_req.read().ok().and_then(|g| g.clone());
                let decision = {
                    let guard = adblock_req.read();
                    match guard {
                        Ok(engine) => {
                            engine.check_request(&uri, page_host.as_deref(), res_type, is_document)
                        }
                        Err(_) => None,
                    }
                };

                if let Some(decision) = decision {
                    if let Ok(response) = env_clone.CreateWebResourceResponse(
                        None,
                        204,
                        w!("Blocked by Ferrum Shield"),
                        w!("Content-Type: text/plain\r\nAccess-Control-Allow-Origin: *\r\n"),
                    ) {
                        let _ = args.SetResponse(&response);
                    }
                    let tab_id = tab_id_req.load(Ordering::Relaxed);
                    if tab_id > 0 {
                        if let Ok(mut engine) = adblock_req.write() {
                            engine.record_blocked(tab_id, &uri, &decision);
                        }
                        let _ = proxy_req.send_event(BrowserEvent::RequestBlocked { tab_id });
                    }
                }

                Ok(())
            })),
            &mut token,
        );

        // 2. SPA-aware URL & back/forward history synchronization
        let proxy_hist = proxy.clone();
        let tab_id_hist = tab_id_cell.clone();
        let tab_host_hist = tab_host.clone();
        let _ = core.add_HistoryChanged(
            &HistoryChangedEventHandler::create(Box::new(move |sender, _| {
                let Some(wv) = sender else { return Ok(()) };
                let tab_id = tab_id_hist.load(Ordering::Relaxed);
                if tab_id == 0 {
                    return Ok(());
                }
                let url = read_webview_source(&wv);
                if let Ok(mut h) = tab_host_hist.write() {
                    *h = extract_host(&url);
                }
                let nav = read_nav_state(&wv);
                let _ = proxy_hist.send_event(BrowserEvent::TabNavUpdated {
                    tab_id,
                    url,
                    can_go_back: nav.can_go_back,
                    can_go_forward: nav.can_go_forward,
                });
                Ok(())
            })),
            &mut token,
        );

        let proxy_src = proxy.clone();
        let tab_id_src = tab_id_cell.clone();
        let tab_host_src = tab_host;
        let _ = core.add_SourceChanged(
            &SourceChangedEventHandler::create(Box::new(move |sender, _| {
                let Some(wv) = sender else { return Ok(()) };
                let tab_id = tab_id_src.load(Ordering::Relaxed);
                if tab_id == 0 {
                    return Ok(());
                }
                let url = read_webview_source(&wv);
                if let Ok(mut h) = tab_host_src.write() {
                    *h = extract_host(&url);
                }
                let nav = read_nav_state(&wv);
                let _ = proxy_src.send_event(BrowserEvent::TabNavUpdated {
                    tab_id,
                    url,
                    can_go_back: nav.can_go_back,
                    can_go_forward: nav.can_go_forward,
                });
                Ok(())
            })),
            &mut token,
        );

        // 3. Per-tab audio & mute state
        if let Ok(core8) = core.cast::<ICoreWebView2_8>() {
            let proxy_audio = proxy.clone();
            let tab_id_audio = tab_id_cell.clone();
            let _ = core8.add_IsDocumentPlayingAudioChanged(
                &IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |sender, _| {
                    let Some(wv) = sender else { return Ok(()) };
                    let tab_id = tab_id_audio.load(Ordering::Relaxed);
                    if tab_id == 0 {
                        return Ok(());
                    }
                    if let Ok(w8) = wv.cast::<ICoreWebView2_8>() {
                        let mut playing = BOOL::default();
                        let mut muted = BOOL::default();
                        let _ = w8.IsDocumentPlayingAudio(&mut playing);
                        let _ = w8.IsMuted(&mut muted);
                        let _ = proxy_audio.send_event(BrowserEvent::TabAudioChanged {
                            tab_id,
                            playing: playing.as_bool(),
                            muted: muted.as_bool(),
                        });
                    }
                    Ok(())
                })),
                &mut token,
            );

            let proxy_mute = proxy.clone();
            let tab_id_mute = tab_id_cell.clone();
            let _ = core8.add_IsMutedChanged(
                &IsMutedChangedEventHandler::create(Box::new(move |sender, _| {
                    let Some(wv) = sender else { return Ok(()) };
                    let tab_id = tab_id_mute.load(Ordering::Relaxed);
                    if tab_id == 0 {
                        return Ok(());
                    }
                    if let Ok(w8) = wv.cast::<ICoreWebView2_8>() {
                        let mut playing = BOOL::default();
                        let mut muted = BOOL::default();
                        let _ = w8.IsDocumentPlayingAudio(&mut playing);
                        let _ = w8.IsMuted(&mut muted);
                        let _ = proxy_mute.send_event(BrowserEvent::TabAudioChanged {
                            tab_id,
                            playing: playing.as_bool(),
                            muted: muted.as_bool(),
                        });
                    }
                    Ok(())
                })),
                &mut token,
            );
        }

        // 4. Native HTML5 video fullscreen integration
        let proxy_fs = proxy.clone();
        let _ = core.add_ContainsFullScreenElementChanged(
            &ContainsFullScreenElementChangedEventHandler::create(Box::new(move |sender, _| {
                let Some(wv) = sender else { return Ok(()) };
                let mut is_fs = BOOL::default();
                let _ = wv.ContainsFullScreenElement(&mut is_fs);
                let _ = proxy_fs.send_event(BrowserEvent::HtmlFullscreenChanged(is_fs.as_bool()));
                Ok(())
            })),
            &mut token,
        );

        // 5. Real website favicons via native Chromium ICoreWebView2_15
        if let Ok(core15) = core.cast::<ICoreWebView2_15>() {
            let proxy_fav = proxy.clone();
            let tab_id_fav = tab_id_cell;
            let _ = core15.add_FaviconChanged(
                &FaviconChangedEventHandler::create(Box::new(move |sender, _| {
                    let Some(wv) = sender else { return Ok(()) };
                    let tab_id = tab_id_fav.load(Ordering::Relaxed);
                    if tab_id == 0 {
                        return Ok(());
                    }
                    if let Ok(w15) = wv.cast::<ICoreWebView2_15>() {
                        let mut raw = PWSTR::null();
                        if w15.FaviconUri(&mut raw).is_ok() {
                            let uri = take_pwstr(raw);
                            let favicon = if uri.is_empty() { None } else { Some(uri) };
                            let _ = proxy_fav
                                .send_event(BrowserEvent::TabFaviconChanged { tab_id, favicon });
                        }
                    }
                    Ok(())
                })),
                &mut token,
            );
        }

        // 6. Native Downloads with real-time byte progress + Pause / Resume / Cancel
        if let Ok(core4) = core.cast::<ICoreWebView2_4>() {
            let proxy_dl = proxy.clone();
            let ops_map = download_ops;
            let _ = core4.add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let op = args.DownloadOperation()?;

                    let uri = {
                        let mut raw = PWSTR::null();
                        op.Uri(&mut raw)?;
                        take_pwstr(raw)
                    };

                    let suggested_path = {
                        let mut raw = PWSTR::null();
                        args.ResultFilePath(&mut raw)?;
                        take_pwstr(raw)
                    };
                    let target = BrowserStorage::unique_download_path(&suggested_path);
                    let target_h = HSTRING::from(target.to_string_lossy().as_ref());
                    let _ = args.SetResultFilePath(&target_h);
                    let _ = args.SetHandled(true);

                    let mut total_i64: i64 = -1;
                    let _ = op.TotalBytesToReceive(&mut total_i64);
                    let total = if total_i64 > 0 {
                        Some(total_i64 as u64)
                    } else {
                        None
                    };

                    let dl_id = generate_id("dl");
                    ops_map.borrow_mut().insert(dl_id.clone(), op.clone());

                    let _ = proxy_dl.send_event(BrowserEvent::DownloadStarted {
                        id: dl_id.clone(),
                        url: uri,
                        path: target.clone(),
                        total_bytes: total,
                    });

                    let dl_id_prog = dl_id.clone();
                    let proxy_prog = proxy_dl.clone();
                    let mut ptok: i64 = 0;
                    let _ = op.add_BytesReceivedChanged(
                        &BytesReceivedChangedEventHandler::create(Box::new(move |op_opt, _| {
                            let Some(o) = op_opt else { return Ok(()) };
                            let mut rec: i64 = 0;
                            let mut tot: i64 = -1;
                            let _ = o.BytesReceived(&mut rec);
                            let _ = o.TotalBytesToReceive(&mut tot);
                            let _ = proxy_prog.send_event(BrowserEvent::DownloadProgress {
                                id: dl_id_prog.clone(),
                                received: rec.max(0) as u64,
                                total: if tot > 0 { Some(tot as u64) } else { None },
                            });
                            Ok(())
                        })),
                        &mut ptok,
                    );

                    let dl_id_state = dl_id;
                    let proxy_state = proxy_dl.clone();
                    let ops_cleanup = ops_map.clone();
                    let mut stok: i64 = 0;
                    let _ = op.add_StateChanged(
                        &StateChangedEventHandler::create(Box::new(move |op_opt, _| {
                            let Some(o) = op_opt else { return Ok(()) };
                            let mut st = COREWEBVIEW2_DOWNLOAD_STATE::default();
                            let _ = o.State(&mut st);

                            let status = if st == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                                ops_cleanup.borrow_mut().remove(&dl_id_state);
                                DownloadStatus::Completed
                            } else if st == COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS {
                                DownloadStatus::InProgress
                            } else if st == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                                let mut can_resume = BOOL::default();
                                let _ = o.CanResume(&mut can_resume);
                                if can_resume.as_bool() {
                                    DownloadStatus::Paused
                                } else {
                                    ops_cleanup.borrow_mut().remove(&dl_id_state);
                                    DownloadStatus::Cancelled
                                }
                            } else {
                                ops_cleanup.borrow_mut().remove(&dl_id_state);
                                DownloadStatus::Failed
                            };

                            let mut final_path = None;
                            if status == DownloadStatus::Completed {
                                let mut raw = PWSTR::null();
                                if o.ResultFilePath(&mut raw).is_ok() {
                                    final_path = Some(PathBuf::from(take_pwstr(raw)));
                                }
                            }

                            let _ = proxy_state.send_event(BrowserEvent::DownloadStateChanged {
                                id: dl_id_state.clone(),
                                status,
                                path: final_path,
                            });
                            Ok(())
                        })),
                        &mut stok,
                    );

                    Ok(())
                })),
                &mut token,
            );
        }

        // 7. Custom Native Ferrum Context Menu (strips Edge clutter, adds Ferrum actions!)
        if let (Ok(core11), Ok(env9)) = (
            core.cast::<ICoreWebView2_11>(),
            env.cast::<ICoreWebView2Environment9>(),
        ) {
            let proxy_ctx = proxy.clone();
            let _ = core11.add_ContextMenuRequested(
                &ContextMenuRequestedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let Ok(items) = args.MenuItems() else {
                        return Ok(());
                    };
                    let Ok(target) = args.ContextMenuTarget() else {
                        return Ok(());
                    };

                    // Strip Edge-specific clutter from the native menu
                    let mut count: u32 = 0;
                    let _ = items.Count(&mut count);
                    for i in (0..count).rev() {
                        if let Ok(item) = items.GetValueAtIndex(i) {
                            let mut raw = PWSTR::null();
                            let name = if item.Name(&mut raw).is_ok() {
                                take_pwstr(raw)
                            } else {
                                String::new()
                            };
                            let lower = name.to_ascii_lowercase();
                            let drop_edge = matches!(
                                lower.as_str(),
                                "share"
                                    | "webcapture"
                                    | "webselect"
                                    | "sidebarsearch"
                                    | "searchinsidebar"
                                    | "readaloud"
                                    | "openlinkinnewwindow"
                                    | "moretools"
                                    | "visualsearch"
                                    | "searchwebfor"
                            ) || lower.contains("edge")
                                || lower.contains("copilot")
                                || lower.contains("sidebar");
                            if drop_edge {
                                let _ = items.RemoveValueAtIndex(i);
                            }
                        }
                    }

                    let add_item = |label: &str,
                                    insert_at: Option<u32>,
                                    action: ContextMenuAction| {
                        let lbl = HSTRING::from(label);
                        if let Ok(mi) = env9.CreateContextMenuItem(
                            &lbl,
                            None,
                            COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
                        ) {
                            let p = proxy_ctx.clone();
                            let mut ctok: i64 = 0;
                            let _ = mi.add_CustomItemSelected(
                                &CustomItemSelectedEventHandler::create(Box::new(move |_, _| {
                                    let _ =
                                        p.send_event(BrowserEvent::ContextAction(action.clone()));
                                    Ok(())
                                })),
                                &mut ctok,
                            );
                            let idx = match insert_at {
                                Some(pos) => pos,
                                None => {
                                    let mut c = 0u32;
                                    let _ = items.Count(&mut c);
                                    c
                                }
                            };
                            let _ = items.InsertValueAtIndex(idx, &mi);
                        }
                    };

                    let add_sep = |insert_at: Option<u32>| {
                        if let Ok(sep) = env9.CreateContextMenuItem(
                            w!(""),
                            None,
                            COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR,
                        ) {
                            let idx = match insert_at {
                                Some(pos) => pos,
                                None => {
                                    let mut c = 0u32;
                                    let _ = items.Count(&mut c);
                                    c
                                }
                            };
                            let _ = items.InsertValueAtIndex(idx, &sep);
                        }
                    };

                    let mut has_link = BOOL::default();
                    let _ = target.HasLinkUri(&mut has_link);
                    if has_link.as_bool() {
                        let mut raw = PWSTR::null();
                        if target.LinkUri(&mut raw).is_ok() {
                            let uri = take_pwstr(raw);
                            if !uri.is_empty() {
                                add_item(
                                    "Abrir enlace en nueva pestaña",
                                    Some(0),
                                    ContextMenuAction::OpenLinkNewTab(uri.clone()),
                                );
                                add_item(
                                    "Abrir en Vista Dividida",
                                    Some(1),
                                    ContextMenuAction::OpenLinkSplit(uri.clone()),
                                );
                                add_item(
                                    "Abrir en pestaña privada",
                                    Some(2),
                                    ContextMenuAction::OpenLinkPrivate(uri.clone()),
                                );
                                add_item(
                                    "Copiar enlace limpio (sin rastreo)",
                                    Some(3),
                                    ContextMenuAction::CopyCleanLink(uri),
                                );
                                add_sep(Some(4));
                            }
                        }
                    }

                    let mut has_sel = BOOL::default();
                    let _ = target.HasSelection(&mut has_sel);
                    if has_sel.as_bool() {
                        let mut raw = PWSTR::null();
                        if target.SelectionText(&mut raw).is_ok() {
                            let sel = take_pwstr(raw).trim().to_string();
                            if !sel.is_empty() {
                                add_sep(None);
                                add_item(
                                    "Buscar selección en Ferrum",
                                    None,
                                    ContextMenuAction::SearchSelection(sel.clone()),
                                );
                                add_item(
                                    "Traducir selección al español",
                                    None,
                                    ContextMenuAction::TranslateSelection(sel.clone()),
                                );
                                add_item(
                                    "Guardar cita en Notas de Ferrum",
                                    None,
                                    ContextMenuAction::SaveSelectionToNotes(sel),
                                );
                            }
                        }
                    }

                    add_sep(None);
                    add_item(
                        "Bloquear este elemento (Escudo)",
                        None,
                        ContextMenuAction::StartElementPicker,
                    );
                    add_item(
                        "Captura de pantalla PNG",
                        None,
                        ContextMenuAction::CaptureScreenshot,
                    );

                    Ok(())
                })),
                &mut token,
            );
        }

        fn read_webview_source(
            wv: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
        ) -> String {
            unsafe {
                let mut raw = PWSTR::null();
                if wv.Source(&mut raw).is_ok() {
                    take_pwstr(raw)
                } else {
                    String::new()
                }
            }
        }

        fn read_nav_state(
            wv: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
        ) -> NavState {
            unsafe {
                let mut back = BOOL::default();
                let mut fwd = BOOL::default();
                let _ = wv.CanGoBack(&mut back);
                let _ = wv.CanGoForward(&mut fwd);
                NavState {
                    can_go_back: back.as_bool(),
                    can_go_forward: fwd.as_bool(),
                }
            }
        }
    }

    attach_accelerator_hooks(webview, proxy);
}

#[cfg(target_os = "windows")]
pub fn attach_accelerator_hooks(webview: &WebView, proxy: EventLoopProxy<BrowserEvent>) {
    use webview2_com::AcceleratorKeyPressedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
        COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, VK_ADD, VK_CONTROL, VK_ESCAPE, VK_F5, VK_F11, VK_F12, VK_LEFT, VK_MENU,
        VK_NUMPAD0, VK_OEM_5, VK_OEM_MINUS, VK_OEM_PLUS, VK_RIGHT, VK_SHIFT, VK_SUBTRACT, VK_TAB,
    };
    use wry::WebViewExtWindows;

    let controller = webview.controller();
    unsafe {
        let mut token: i64 = 0;
        let _ = controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                    && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                {
                    return Ok(());
                }

                let mut vk: u32 = 0;
                args.VirtualKey(&mut vk)?;

                let ctrl = (GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;
                let shift = (GetKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0;
                let alt = (GetKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0;

                let action = if ctrl && !alt {
                    match vk {
                        k if k == b'T' as u32 && !shift => Some(ShortcutAction::NewTab),
                        k if k == b'T' as u32 && shift => Some(ShortcutAction::ReopenClosedTab),
                        k if k == b'N' as u32 && !shift => Some(ShortcutAction::NewWindow),
                        k if k == b'N' as u32 && shift => Some(ShortcutAction::NewPrivateTab),
                        k if k == b'W' as u32 && !shift => Some(ShortcutAction::CloseTab),
                        k if k == b'K' as u32 && !shift => Some(ShortcutAction::OpenCommands),
                        k if k == b'K' as u32 && shift => Some(ShortcutAction::DuplicateTab),
                        k if k == b'P' as u32 && !shift => Some(ShortcutAction::PrintPage),
                        k if k == b'P' as u32 && shift => Some(ShortcutAction::TogglePinTab),
                        k if k == b'S' as u32 && shift => Some(ShortcutAction::CaptureScreenshot),
                        k if k == b'C' as u32 && shift => Some(ShortcutAction::CopyCleanUrl),
                        k if k == b'M' as u32 && !shift => Some(ShortcutAction::ToggleMuteTab),
                        k if k == b'M' as u32 && shift => {
                            Some(ShortcutAction::MoveActiveTabToNewWindow)
                        }
                        k if k == b'L' as u32 && !shift => Some(ShortcutAction::FocusOmnibox),
                        k if k == b'L' as u32 && shift => Some(ShortcutAction::QuickAutofill),
                        k if k == b'F' as u32 && !shift => Some(ShortcutAction::OpenFind),
                        k if k == b'J' as u32 && !shift => Some(ShortcutAction::ToggleDownloads),
                        k if k == b'R' as u32 && !shift => Some(ShortcutAction::Reload),
                        k if k == b'H' as u32 && !shift => Some(ShortcutAction::ToggleHistory),
                        k if k == b'D' as u32 && !shift => Some(ShortcutAction::ToggleBookmark),
                        k if k == b'B' as u32 && shift => Some(ShortcutAction::ToggleBookmarksBar),
                        k if k == VK_OEM_5.0 as u32 && !shift => {
                            Some(ShortcutAction::ToggleSplitView)
                        }
                        k if (k == VK_OEM_PLUS.0 as u32 || k == VK_ADD.0 as u32) => {
                            Some(ShortcutAction::ZoomIn)
                        }
                        k if (k == VK_OEM_MINUS.0 as u32 || k == VK_SUBTRACT.0 as u32) => {
                            Some(ShortcutAction::ZoomOut)
                        }
                        k if (k == b'0' as u32 || k == VK_NUMPAD0.0 as u32) && !shift => {
                            Some(ShortcutAction::ZoomReset)
                        }
                        k if (b'1' as u32..=b'9' as u32).contains(&k) && !shift => {
                            Some(ShortcutAction::JumpToTab((k - b'1' as u32) as usize))
                        }
                        k if k == VK_TAB.0 as u32 && !shift => Some(ShortcutAction::NextTab),
                        k if k == VK_TAB.0 as u32 && shift => Some(ShortcutAction::PrevTab),
                        _ => None,
                    }
                } else if alt && !ctrl && !shift {
                    match vk {
                        k if k == VK_LEFT.0 as u32 => Some(ShortcutAction::GoBack),
                        k if k == VK_RIGHT.0 as u32 => Some(ShortcutAction::GoForward),
                        k if k == b'R' as u32 => Some(ShortcutAction::ToggleReaderMode),
                        k if k == b'D' as u32 => Some(ShortcutAction::ToggleForceDark),
                        k if k == b'P' as u32 => Some(ShortcutAction::TogglePiP),
                        k if k == b'E' as u32 => Some(ShortcutAction::StartElementPicker),
                        _ => None,
                    }
                } else if ctrl && alt && !shift {
                    match vk {
                        k if (b'1' as u32..=b'9' as u32).contains(&k) => Some(
                            ShortcutAction::SwitchWorkspaceIndex((k - b'1' as u32) as usize),
                        ),
                        _ => None,
                    }
                } else if shift && !ctrl && !alt && vk == VK_ESCAPE.0 as u32 {
                    Some(ShortcutAction::OpenTaskManager)
                } else if !ctrl && !alt && !shift {
                    match vk {
                        k if k == VK_F5.0 as u32 => Some(ShortcutAction::Reload),
                        k if k == VK_F11.0 as u32 => Some(ShortcutAction::ToggleFullscreen),
                        k if k == VK_F12.0 as u32 => Some(ShortcutAction::OpenDevtools),
                        k if k == VK_ESCAPE.0 as u32 => Some(ShortcutAction::EscapePressed),
                        _ => None,
                    }
                } else {
                    None
                };

                if let Some(action) = action {
                    if !matches!(action, ShortcutAction::EscapePressed) {
                        let _ = args.SetHandled(true);
                    }
                    let _ = proxy.send_event(BrowserEvent::Shortcut(action));
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

/// Deletes all cookies & web storage for the active site only (via `ICoreWebView2_2::CookieManager`).
#[cfg(target_os = "windows")]
pub fn clear_site_cookies(
    webview: &WebView,
    site_url: &str,
    host_label: String,
    proxy: EventLoopProxy<BrowserEvent>,
) {
    use webview2_com::GetCookiesCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_2;
    use windows_core::{HSTRING, Interface};
    use wry::WebViewExtWindows;

    let _ = webview
        .evaluate_script("try { localStorage.clear(); sessionStorage.clear(); } catch(_) {}");

    let core = webview.webview();
    unsafe {
        let Ok(core2) = core.cast::<ICoreWebView2_2>() else {
            let _ = proxy.send_event(BrowserEvent::Toast(format!(
                "Almacenamiento local borrado en {host_label}"
            )));
            return;
        };
        let Ok(mgr) = core2.CookieManager() else {
            return;
        };
        let mgr_del = mgr.clone();
        let uri_h = HSTRING::from(site_url);
        let _ = mgr.GetCookies(
            &uri_h,
            &GetCookiesCompletedHandler::create(Box::new(move |_hr, list_opt| {
                let mut deleted = 0u32;
                if let Some(list) = list_opt {
                    let mut count = 0u32;
                    let _ = list.Count(&mut count);
                    for i in 0..count {
                        if let Ok(cookie) = list.GetValueAtIndex(i) {
                            let _ = mgr_del.DeleteCookie(&cookie);
                            deleted += 1;
                        }
                    }
                }
                let _ = proxy.send_event(BrowserEvent::Toast(format!(
                    "Eliminadas {deleted} cookies y datos de {host_label}"
                )));
                Ok(())
            })),
        );
    }
}

/// Loads an unpacked Chromium MV3 extension folder into WebView2 Profile7.
#[cfg(target_os = "windows")]
pub fn add_browser_extension(
    webview: &WebView,
    folder_path: &str,
    proxy: EventLoopProxy<BrowserEvent>,
) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2_13, ICoreWebView2Profile7};
    use webview2_com::{ProfileAddBrowserExtensionCompletedHandler, take_pwstr};
    use windows_core::{HSTRING, Interface, PWSTR};
    use wry::WebViewExtWindows;

    let core = webview.webview();
    let path_h = HSTRING::from(folder_path);
    unsafe {
        let Ok(core13) = core.cast::<ICoreWebView2_13>() else {
            let _ = proxy.send_event(BrowserEvent::Toast(
                "Tu versión de WebView2 no soporta extensiones.".into(),
            ));
            return;
        };
        let Ok(profile) = core13.Profile() else {
            return;
        };
        let Ok(profile7) = profile.cast::<ICoreWebView2Profile7>() else {
            let _ = proxy.send_event(BrowserEvent::Toast(
                "Extensiones no disponibles en este perfil.".into(),
            ));
            return;
        };
        let proxy_done = proxy.clone();
        let profile_refresh = profile7.clone();
        let _ = profile7.AddBrowserExtension(
            &path_h,
            &ProfileAddBrowserExtensionCompletedHandler::create(Box::new(move |hr, ext_opt| {
                if hr.is_ok() {
                    let name = ext_opt
                        .and_then(|e| {
                            let mut raw = PWSTR::null();
                            e.Name(&mut raw).ok().map(|_| take_pwstr(raw))
                        })
                        .unwrap_or_else(|| "Extensión".into());
                    let _ = proxy_done
                        .send_event(BrowserEvent::Toast(format!("Extensión cargada: {name}")));
                    query_profile_extensions(&profile_refresh, proxy_done.clone());
                } else {
                    let _ = proxy_done.send_event(BrowserEvent::Toast(
                        "No se pudo cargar la extensión (verifica que la carpeta contenga manifest.json)."
                            .into(),
                    ));
                }
                Ok(())
            })),
        );
    }
}

#[cfg(target_os = "windows")]
pub fn list_browser_extensions(webview: &WebView, proxy: EventLoopProxy<BrowserEvent>) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2_13, ICoreWebView2Profile7};
    use windows_core::Interface;
    use wry::WebViewExtWindows;

    unsafe {
        if let Ok(core13) = webview.webview().cast::<ICoreWebView2_13>()
            && let Ok(profile) = core13.Profile()
            && let Ok(profile7) = profile.cast::<ICoreWebView2Profile7>()
        {
            query_profile_extensions(&profile7, proxy);
        }
    }
}

#[cfg(target_os = "windows")]
pub fn toggle_browser_extension(
    webview: &WebView,
    ext_id: String,
    enable: bool,
    proxy: EventLoopProxy<BrowserEvent>,
) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2_13, ICoreWebView2Profile7};
    use webview2_com::{
        BrowserExtensionEnableCompletedHandler, ProfileGetBrowserExtensionsCompletedHandler,
        take_pwstr,
    };
    use windows_core::{Interface, PWSTR};
    use wry::WebViewExtWindows;

    unsafe {
        if let Ok(core13) = webview.webview().cast::<ICoreWebView2_13>()
            && let Ok(profile) = core13.Profile()
            && let Ok(profile7) = profile.cast::<ICoreWebView2Profile7>()
        {
            let p7 = profile7.clone();
            let _ = profile7.GetBrowserExtensions(
                &ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(
                    move |_hr, list_opt| {
                        if let Some(list) = list_opt {
                            let mut count = 0u32;
                            let _ = list.Count(&mut count);
                            for i in 0..count {
                                if let Ok(ext) = list.GetValueAtIndex(i) {
                                    let mut raw = PWSTR::null();
                                    let id = if ext.Id(&mut raw).is_ok() {
                                        take_pwstr(raw)
                                    } else {
                                        continue;
                                    };
                                    if id == ext_id {
                                        let p_ref = p7.clone();
                                        let px = proxy.clone();
                                        let _ = ext.Enable(
                                            enable,
                                            &BrowserExtensionEnableCompletedHandler::create(
                                                Box::new(move |_| {
                                                    query_profile_extensions(&p_ref, px.clone());
                                                    Ok(())
                                                }),
                                            ),
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                        Ok(())
                    },
                )),
            );
        }
    }
}

#[cfg(target_os = "windows")]
pub fn remove_browser_extension(
    webview: &WebView,
    ext_id: String,
    proxy: EventLoopProxy<BrowserEvent>,
) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2_13, ICoreWebView2Profile7};
    use webview2_com::{
        BrowserExtensionRemoveCompletedHandler, ProfileGetBrowserExtensionsCompletedHandler,
        take_pwstr,
    };
    use windows_core::{Interface, PWSTR};
    use wry::WebViewExtWindows;

    unsafe {
        if let Ok(core13) = webview.webview().cast::<ICoreWebView2_13>()
            && let Ok(profile) = core13.Profile()
            && let Ok(profile7) = profile.cast::<ICoreWebView2Profile7>()
        {
            let p7 = profile7.clone();
            let _ = profile7.GetBrowserExtensions(
                &ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(
                    move |_hr, list_opt| {
                        if let Some(list) = list_opt {
                            let mut count = 0u32;
                            let _ = list.Count(&mut count);
                            for i in 0..count {
                                if let Ok(ext) = list.GetValueAtIndex(i) {
                                    let mut raw = PWSTR::null();
                                    let id = if ext.Id(&mut raw).is_ok() {
                                        take_pwstr(raw)
                                    } else {
                                        continue;
                                    };
                                    if id == ext_id {
                                        let p_ref = p7.clone();
                                        let px = proxy.clone();
                                        let _ = ext.Remove(
                                            &BrowserExtensionRemoveCompletedHandler::create(
                                                Box::new(move |_| {
                                                    let _ = px.send_event(BrowserEvent::Toast(
                                                        "Extensión eliminada".into(),
                                                    ));
                                                    query_profile_extensions(&p_ref, px.clone());
                                                    Ok(())
                                                }),
                                            ),
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                        Ok(())
                    },
                )),
            );
        }
    }
}

#[cfg(target_os = "windows")]
unsafe fn query_profile_extensions(
    profile7: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Profile7,
    proxy: EventLoopProxy<BrowserEvent>,
) {
    use webview2_com::{ProfileGetBrowserExtensionsCompletedHandler, take_pwstr};
    use windows_core::{BOOL, PWSTR};

    unsafe {
        let _ = profile7.GetBrowserExtensions(
            &ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |_hr, list_opt| {
                let mut out = Vec::new();
                if let Some(list) = list_opt {
                    let mut count = 0u32;
                    let _ = list.Count(&mut count);
                    for i in 0..count {
                        if let Ok(ext) = list.GetValueAtIndex(i) {
                            let mut raw_id = PWSTR::null();
                            let mut raw_name = PWSTR::null();
                            let mut enabled = BOOL::default();
                            let id = if ext.Id(&mut raw_id).is_ok() {
                                take_pwstr(raw_id)
                            } else {
                                continue;
                            };
                            let name = if ext.Name(&mut raw_name).is_ok() {
                                take_pwstr(raw_name)
                            } else {
                                id.clone()
                            };
                            let _ = ext.IsEnabled(&mut enabled);
                            out.push(ChromiumExtensionInfo {
                                id,
                                name,
                                enabled: enabled.as_bool(),
                            });
                        }
                    }
                }
                let _ = proxy.send_event(BrowserEvent::ExtensionsUpdated(out));
                Ok(())
            })),
        );
    }
}

#[cfg(target_os = "windows")]
pub fn enable_clip_siblings(webview: &WebView) {
    use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject};
    use windows::Win32::UI::WindowsAndMessaging::{
        GCLP_HBRBACKGROUND, GWL_STYLE, GetWindowLongPtrW, SetClassLongPtrW, SetWindowLongPtrW,
        WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
    };
    use wry::WebViewExtWindows;

    let hwnd = webview.hwnd();
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let next = style | (WS_CLIPSIBLINGS.0 as isize) | (WS_CLIPCHILDREN.0 as isize);
        if next != style {
            SetWindowLongPtrW(hwnd, GWL_STYLE, next);
        }
        let black = GetStockObject(BLACK_BRUSH);
        if !black.is_invalid() {
            SetClassLongPtrW(hwnd, GCLP_HBRBACKGROUND, black.0 as isize);
        }
    }
}

#[cfg(target_os = "windows")]
pub fn set_webview_bounds_physical(webview: &WebView, x: i32, y: i32, w: i32, h: i32) {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos};
    use wry::WebViewExtWindows;

    let w = w.max(0);
    let h = h.max(0);

    unsafe {
        let _ = SetWindowPos(
            webview.hwnd(),
            None,
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        let controller = webview.controller();
        let _ = controller.SetBounds(RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        });
        let _ = controller.NotifyParentWindowPositionChanged();
    }
}

#[cfg(target_os = "windows")]
pub fn set_webview_bounds_sync(webview: &WebView, bounds: wry::Rect, scale_factor: f64) {
    let size = bounds.size.to_physical::<i32>(scale_factor);
    let pos = bounds.position.to_physical::<i32>(scale_factor);
    set_webview_bounds_physical(webview, pos.x, pos.y, size.width, size.height);
}

#[cfg(target_os = "windows")]
pub fn notify_parent_moved(webview: &WebView) {
    use wry::WebViewExtWindows;
    unsafe {
        let _ = webview.controller().NotifyParentWindowPositionChanged();
    }
}

#[cfg(target_os = "windows")]
pub fn show_webview_smooth(webview: &WebView) {
    use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNA, ShowWindow};
    use wry::WebViewExtWindows;

    unsafe {
        let _ = webview.controller().SetIsVisible(true);
        let _ = ShowWindow(webview.hwnd(), SW_SHOWNA);
    }
}

#[cfg(target_os = "windows")]
pub fn hide_webview_smooth(webview: &WebView) {
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
    use wry::WebViewExtWindows;

    unsafe {
        let _ = ShowWindow(webview.hwnd(), SW_HIDE);
        let _ = webview.controller().SetIsVisible(false);
    }
}

/// Suspends a hidden WebView2 via `ICoreWebView2_3::TrySuspend` so Chromium frees V8/compositor
/// memory while preserving full DOM/scroll state for instant (~1-3ms) `resume_webview()`.
#[cfg(target_os = "windows")]
pub fn suspend_webview(webview: &WebView) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
    use webview2_com::TrySuspendCompletedHandler;
    use windows_core::Interface;
    use wry::WebViewExtWindows;

    unsafe {
        if let Ok(core3) = webview.webview().cast::<ICoreWebView2_3>() {
            let _ = core3.TrySuspend(&TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(()))));
        }
    }
}

/// Resumes a previously suspended WebView2 instantaneously without reloading the page.
#[cfg(target_os = "windows")]
pub fn resume_webview(webview: &WebView) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
    use windows_core::Interface;
    use wry::WebViewExtWindows;

    unsafe {
        if let Ok(core3) = webview.webview().cast::<ICoreWebView2_3>() {
            let _ = core3.Resume();
        }
    }
}

#[cfg(target_os = "windows")]
pub fn destroy_webview_smooth(webview: WebView) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
    };
    use wry::WebViewExtWindows;

    hide_webview_smooth(&webview);
    unsafe {
        let _ = SetWindowPos(
            webview.hwnd(),
            None,
            -32000,
            -32000,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_HIDEWINDOW,
        );
    }
    drop(webview);
}

#[cfg(target_os = "windows")]
pub fn bring_webview_to_top(webview: &WebView) {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };
    use wry::WebViewExtWindows;

    unsafe {
        let _ = SetWindowPos(
            webview.hwnd(),
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

#[cfg(target_os = "windows")]
pub fn bring_window_to_foreground(window: &tao::window::Window) {
    use tao::platform::windows::WindowExtWindows;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };

    let hwnd = HWND(window.hwnd() as _);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

#[cfg(target_os = "windows")]
pub fn capture_screenshot_png(
    webview: &WebView,
    target_path: PathBuf,
    proxy: EventLoopProxy<BrowserEvent>,
) {
    use webview2_com::CapturePreviewCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
    use windows::Win32::System::Com::{STGM_CREATE, STGM_SHARE_DENY_WRITE, STGM_WRITE};
    use windows::Win32::UI::Shell::SHCreateStreamOnFileEx;
    use windows_core::HSTRING;
    use wry::WebViewExtWindows;

    let core = webview.webview();
    let path_h = HSTRING::from(target_path.to_string_lossy().as_ref());
    unsafe {
        let mode = (STGM_CREATE | STGM_WRITE | STGM_SHARE_DENY_WRITE).0;
        let Ok(stream) = SHCreateStreamOnFileEx(&path_h, mode, 0, true, None) else {
            let _ = proxy.send_event(BrowserEvent::Toast(
                "No se pudo crear el archivo de captura.".to_string(),
            ));
            return;
        };

        let _ = core.CapturePreview(
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
            &stream,
            &CapturePreviewCompletedHandler::create(Box::new(move |hr| {
                if hr.is_ok() {
                    let _ = proxy.send_event(BrowserEvent::ScreenshotSaved(target_path.clone()));
                } else {
                    let _ = proxy.send_event(BrowserEvent::Toast(
                        "Error al capturar pantalla.".to_string(),
                    ));
                }
                Ok(())
            })),
        );
    }
}

#[cfg(target_os = "windows")]
pub fn pause_download(ops: &DownloadOpsMap, id: &str) {
    if let Some(op) = ops.borrow().get(id) {
        unsafe {
            let _ = op.Pause();
        }
    }
}

#[cfg(target_os = "windows")]
pub fn resume_download(ops: &DownloadOpsMap, id: &str) {
    if let Some(op) = ops.borrow().get(id) {
        unsafe {
            let _ = op.Resume();
        }
    }
}

#[cfg(target_os = "windows")]
pub fn cancel_download(ops: &DownloadOpsMap, id: &str) {
    if let Some(op) = ops.borrow().get(id) {
        unsafe {
            let _ = op.Cancel();
        }
    }
}

#[cfg(target_os = "windows")]
pub fn copy_text_to_clipboard(text: &str) -> bool {
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * std::mem::size_of::<u16>();

    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, bytes) else {
            let _ = CloseClipboard();
            return false;
        };
        let ptr = GlobalLock(hmem);
        if !ptr.is_null() {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr as *mut u16, wide.len());
            let _ = GlobalUnlock(hmem);
        }
        let ok = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(HGLOBAL(hmem.0).0))).is_ok();
        let _ = CloseClipboard();
        ok
    }
}

/// Copies a sensitive secret (such as a password) to the clipboard and automatically clears it
/// after `delay_secs` seconds IF AND ONLY IF the clipboard still contains that exact secret.
#[cfg(target_os = "windows")]
pub fn copy_secret_with_auto_clear(secret: &str, delay_secs: u64) -> bool {
    use zeroize::Zeroize;
    if !copy_text_to_clipboard(secret) {
        return false;
    }
    let mut expected = secret.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(delay_secs));
        clear_clipboard_if_matches(&expected);
        expected.zeroize();
    });
    true
}

#[cfg(target_os = "windows")]
fn clear_clipboard_if_matches(expected: &str) {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use zeroize::Zeroize;

    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        if OpenClipboard(None).is_err() {
            return;
        }
        let mut should_clear = false;
        if let Ok(handle) = GetClipboardData(CF_UNICODETEXT) {
            let hglobal = HGLOBAL(handle.0);
            let ptr = GlobalLock(hglobal) as *const u16;
            if !ptr.is_null() {
                let mut len = 0usize;
                while len < 4096 && *ptr.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(ptr, len);
                let mut current = String::from_utf16_lossy(slice);
                should_clear = current == expected;
                current.zeroize();
                let _ = GlobalUnlock(hglobal);
            }
        }
        if should_clear {
            let _ = EmptyClipboard();
        }
        let _ = CloseClipboard();
    }
}

/// Opens a file with its registered Windows handler using `ShellExecuteW` directly (never `cmd.exe`),
/// preventing any shell metacharacter injection via downloaded filenames.
#[cfg(target_os = "windows")]
pub fn open_file_native(path: &str) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows_core::{HSTRING, PCWSTR, w};

    let path_h = HSTRING::from(path);
    unsafe {
        let _ = ShellExecuteW(
            None,
            w!("open"),
            &path_h,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(target_os = "windows")]
pub fn go_back(webview: &WebView) {
    use wry::WebViewExtWindows;
    unsafe {
        let _ = webview.webview().GoBack();
    }
}

#[cfg(target_os = "windows")]
pub fn go_forward(webview: &WebView) {
    use wry::WebViewExtWindows;
    unsafe {
        let _ = webview.webview().GoForward();
    }
}

#[cfg(target_os = "windows")]
pub fn stop_loading(webview: &WebView) {
    use wry::WebViewExtWindows;
    unsafe {
        let _ = webview.webview().Stop();
    }
}

#[cfg(target_os = "windows")]
pub fn set_muted(webview: &WebView, muted: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
    use windows_core::Interface;
    use wry::WebViewExtWindows;
    unsafe {
        if let Ok(core8) = webview.webview().cast::<ICoreWebView2_8>() {
            let _ = core8.SetIsMuted(muted);
        }
    }
}

#[cfg(target_os = "windows")]
pub fn set_zoom_factor(webview: &WebView, factor: f64) {
    use wry::WebViewExtWindows;
    unsafe {
        let _ = webview.controller().SetZoomFactor(factor.clamp(0.5, 3.0));
    }
}

#[cfg(target_os = "windows")]
pub fn query_nav_state(webview: &WebView) -> NavState {
    use windows_core::BOOL;
    use wry::WebViewExtWindows;
    unsafe {
        let core = webview.webview();
        let mut back = BOOL::default();
        let mut fwd = BOOL::default();
        let _ = core.CanGoBack(&mut back);
        let _ = core.CanGoForward(&mut fwd);
        NavState {
            can_go_back: back.as_bool(),
            can_go_forward: fwd.as_bool(),
        }
    }
}

#[cfg(target_os = "windows")]
pub fn apply_windows11_frame_styling(window: &tao::window::Window) {
    use tao::platform::windows::WindowExtWindows;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
        DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject};
    use windows::Win32::UI::WindowsAndMessaging::{
        GCLP_HBRBACKGROUND, GWL_STYLE, GetWindowLongPtrW, SetClassLongPtrW, SetWindowLongPtrW,
        WS_CLIPCHILDREN,
    };

    let hwnd = HWND(window.hwnd() as _);
    unsafe {
        let dark_mode: u32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_mode as *const _ as *const _,
            std::mem::size_of_val(&dark_mode) as u32,
        );
        let corner_pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner_pref as *const _ as *const _,
            std::mem::size_of_val(&corner_pref) as u32,
        );
        let border_color: u32 = 0x0026_2222;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &border_color as *const _ as *const _,
            std::mem::size_of_val(&border_color) as u32,
        );

        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let next = style | (WS_CLIPCHILDREN.0 as isize);
        if next != style {
            SetWindowLongPtrW(hwnd, GWL_STYLE, next);
        }
        let black = GetStockObject(BLACK_BRUSH);
        if !black.is_invalid() {
            SetClassLongPtrW(hwnd, GCLP_HBRBACKGROUND, black.0 as isize);
        }
    }
}

/// Opens a native Windows File/Folder Explorer picker on a dedicated STA background thread
/// so the main event loop stays completely responsive, then returns the picked path via `BrowserEvent`.
#[cfg(target_os = "windows")]
pub fn pick_native_path_async(
    folder_mode: bool,
    title: String,
    filter_label: Option<String>,
    filter_spec: Option<String>,
    target: String,
    proxy: EventLoopProxy<BrowserEvent>,
) {
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize,
    };
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PICKFOLDERS, FileOpenDialog,
        IFileOpenDialog, SIGDN_FILESYSPATH,
    };
    use windows_core::{HSTRING, PCWSTR};

    std::thread::spawn(move || unsafe {
        let init_hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let did_init = init_hr.is_ok();

        let chosen = (|| -> Option<String> {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).ok()?;
            let mut opts = dialog.GetOptions().ok()?;
            opts |= FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR | FOS_FILEMUSTEXIST;
            if folder_mode {
                opts |= FOS_PICKFOLDERS;
            }
            let _ = dialog.SetOptions(opts);
            let title_h = HSTRING::from(title.as_str());
            let _ = dialog.SetTitle(&title_h);

            if !folder_mode && let (Some(lbl), Some(spec)) = (&filter_label, &filter_spec) {
                let lbl_h = HSTRING::from(lbl.as_str());
                let spec_h = HSTRING::from(spec.as_str());
                let filters = [COMDLG_FILTERSPEC {
                    pszName: PCWSTR::from_raw(lbl_h.as_ptr()),
                    pszSpec: PCWSTR::from_raw(spec_h.as_ptr()),
                }];
                let _ = dialog.SetFileTypes(&filters);
            }

            dialog.Show(None).ok()?;
            let item = dialog.GetResult().ok()?;
            let raw = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let path = raw.to_string().ok();
            CoTaskMemFree(Some(raw.0 as _));
            path
        })();

        if did_init {
            CoUninitialize();
        }

        if let Some(path) = chosen
            && !path.trim().is_empty()
        {
            let _ = proxy.send_event(BrowserEvent::NativePathPicked { target, path });
        }
    });
}

#[cfg(not(target_os = "windows"))]
pub fn pick_native_path_async(
    _folder_mode: bool,
    _title: String,
    _filter_label: Option<String>,
    _filter_spec: Option<String>,
    _target: String,
    _proxy: EventLoopProxy<BrowserEvent>,
) {
}

#[cfg(not(target_os = "windows"))]
pub fn apply_windows11_frame_styling(_window: &tao::window::Window) {}
#[cfg(not(target_os = "windows"))]
pub fn attach_content_hooks(
    _webview: &WebView,
    _tab_id_cell: Arc<AtomicU64>,
    _tab_host: Arc<RwLock<Option<String>>>,
    _adblock: Arc<RwLock<AdBlockEngine>>,
    _download_ops: DownloadOpsMap,
    _proxy: EventLoopProxy<BrowserEvent>,
) {
}
#[cfg(not(target_os = "windows"))]
pub fn attach_accelerator_hooks(_webview: &WebView, _proxy: EventLoopProxy<BrowserEvent>) {}
#[cfg(not(target_os = "windows"))]
pub fn clear_site_cookies(
    _webview: &WebView,
    _site_url: &str,
    _host_label: String,
    _proxy: EventLoopProxy<BrowserEvent>,
) {
}
#[cfg(not(target_os = "windows"))]
pub fn add_browser_extension(
    _webview: &WebView,
    _folder_path: &str,
    _proxy: EventLoopProxy<BrowserEvent>,
) {
}
#[cfg(not(target_os = "windows"))]
pub fn list_browser_extensions(_webview: &WebView, _proxy: EventLoopProxy<BrowserEvent>) {}
#[cfg(not(target_os = "windows"))]
pub fn toggle_browser_extension(
    _webview: &WebView,
    _ext_id: String,
    _enable: bool,
    _proxy: EventLoopProxy<BrowserEvent>,
) {
}
#[cfg(not(target_os = "windows"))]
pub fn remove_browser_extension(
    _webview: &WebView,
    _ext_id: String,
    _proxy: EventLoopProxy<BrowserEvent>,
) {
}
#[cfg(not(target_os = "windows"))]
pub fn enable_clip_siblings(_webview: &WebView) {}
#[cfg(not(target_os = "windows"))]
pub fn set_webview_bounds_physical(webview: &WebView, x: i32, y: i32, w: i32, h: i32) {
    let _ = webview.set_bounds(wry::Rect {
        position: wry::dpi::PhysicalPosition::new(x, y).into(),
        size: wry::dpi::PhysicalSize::new(w.max(0) as u32, h.max(0) as u32).into(),
    });
}
#[cfg(not(target_os = "windows"))]
pub fn set_webview_bounds_sync(webview: &WebView, bounds: wry::Rect, _scale_factor: f64) {
    let _ = webview.set_bounds(bounds);
}
#[cfg(not(target_os = "windows"))]
pub fn notify_parent_moved(_webview: &WebView) {}
#[cfg(not(target_os = "windows"))]
pub fn show_webview_smooth(webview: &WebView) {
    let _ = webview.set_visible(true);
}
#[cfg(not(target_os = "windows"))]
pub fn hide_webview_smooth(webview: &WebView) {
    let _ = webview.set_visible(false);
}
#[cfg(not(target_os = "windows"))]
pub fn suspend_webview(_webview: &WebView) {}
#[cfg(not(target_os = "windows"))]
pub fn resume_webview(_webview: &WebView) {}
#[cfg(not(target_os = "windows"))]
pub fn destroy_webview_smooth(webview: WebView) {
    let _ = webview.set_visible(false);
    drop(webview);
}
#[cfg(not(target_os = "windows"))]
pub fn bring_webview_to_top(_webview: &WebView) {}
#[cfg(not(target_os = "windows"))]
pub fn bring_window_to_foreground(_window: &tao::window::Window) {}
#[cfg(not(target_os = "windows"))]
pub fn capture_screenshot_png(
    _webview: &WebView,
    _target_path: PathBuf,
    _proxy: EventLoopProxy<BrowserEvent>,
) {
}
#[cfg(not(target_os = "windows"))]
pub fn pause_download(_ops: &DownloadOpsMap, _id: &str) {}
#[cfg(not(target_os = "windows"))]
pub fn resume_download(_ops: &DownloadOpsMap, _id: &str) {}
#[cfg(not(target_os = "windows"))]
pub fn cancel_download(_ops: &DownloadOpsMap, _id: &str) {}
#[cfg(not(target_os = "windows"))]
pub fn copy_text_to_clipboard(_text: &str) -> bool {
    false
}
#[cfg(not(target_os = "windows"))]
pub fn copy_secret_with_auto_clear(_secret: &str, _delay_secs: u64) -> bool {
    false
}
#[cfg(not(target_os = "windows"))]
pub fn open_file_native(path: &str) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}
#[cfg(not(target_os = "windows"))]
pub fn go_back(webview: &WebView) {
    let _ = webview.evaluate_script("history.back()");
}
#[cfg(not(target_os = "windows"))]
pub fn go_forward(webview: &WebView) {
    let _ = webview.evaluate_script("history.forward()");
}
#[cfg(not(target_os = "windows"))]
pub fn stop_loading(webview: &WebView) {
    let _ = webview.evaluate_script("window.stop()");
}
#[cfg(not(target_os = "windows"))]
pub fn set_muted(_webview: &WebView, _muted: bool) {}
#[cfg(not(target_os = "windows"))]
pub fn set_zoom_factor(_webview: &WebView, _factor: f64) {}
#[cfg(not(target_os = "windows"))]
pub fn query_nav_state(_webview: &WebView) -> NavState {
    NavState::default()
}
