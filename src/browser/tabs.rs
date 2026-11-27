use super::*;

impl BrowserApp {
    pub(super) fn create_tab(
        &mut self,
        initial_url: &str,
        activate: bool,
        is_private: bool,
    ) -> wry::Result<u64> {
        let ws = self.active_workspace_id.clone();
        self.create_tab_full(initial_url, activate, is_private, false, ws)
    }

    pub(super) fn create_hibernated_tab(
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
            is_suspended: false,
            title,
            url: display_url,
            favicon_url: None,
            host_cell: Arc::new(RwLock::new(initial_host)),
            workspace_id,
            group_id: None,
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

    pub(super) fn create_tab_full(
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
            is_suspended: false,
            title: initial_title,
            url: display_url,
            favicon_url: None,
            host_cell,
            workspace_id,
            group_id: None,
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
    pub(super) fn build_tab_webview(
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

    pub(super) fn hibernate_tab(&mut self, id: u64, silent: bool) {
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
        tab.is_suspended = false;
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

    pub(super) fn ensure_tab_awake(&mut self, id: u64) {
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
                tab.is_suspended = false;
                tab.loading = !is_internal_url(&target);
                tab.last_active = Instant::now();
            }
        }
    }

    pub(super) fn reorder_tab(&mut self, from_id: u64, to_id: u64) {
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

    pub(super) fn switch_tab(&mut self, id: u64) {
        let Some((ws_id, grp_id)) = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .map(|t| (t.workspace_id.clone(), t.group_id.clone()))
        else {
            return;
        };

        // Ensure group is expanded if user switches into a collapsed group's tab
        if let Some(gid) = grp_id
            && let Some(g) = self.tab_groups.iter_mut().find(|g| g.id == gid)
        {
            g.collapsed = false;
        }

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

    pub(super) fn close_tab(&mut self, id: u64) {
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
        self.prune_empty_tab_groups();

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

    pub(super) fn close_other_tabs(&mut self, keep_id: u64) {
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

    pub(super) fn duplicate_tab(&mut self, id: u64) {
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

    pub(super) fn toggle_pin_tab(&mut self, id: u64) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.pinned = !tab.pinned;
        }
        self.context_tab_id = None;
        self.save_session_now();
        self.relayout();
        self.ui_dirty = true;
    }

    pub(super) fn toggle_mute_tab(&mut self, id: u64) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.is_muted = !tab.is_muted;
            if let Some(wv) = &tab.webview {
                webview_hooks::set_muted(wv, tab.is_muted);
            }
        }
        self.context_tab_id = None;
        self.ui_dirty = true;
    }

    pub(super) fn toggle_split_view(&mut self, target_tab_id: Option<u64>) {
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

    pub(super) fn switch_workspace(&mut self, workspace_id: &str) {
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

    pub(super) fn cycle_tab(&mut self, delta: isize) {
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

    pub(super) fn jump_to_workspace_tab(&mut self, idx: usize) {
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

    pub(super) fn switch_workspace_by_index(&mut self, idx: usize) {
        if let Some(ws_id) = self
            .storage
            .settings
            .workspaces
            .get(idx)
            .map(|w| w.id.clone())
        {
            self.switch_workspace(&ws_id);
            if let Some(name) = self
                .storage
                .settings
                .workspaces
                .iter()
                .find(|w| w.id == ws_id)
                .map(|w| w.name.clone())
            {
                self.show_toast(format!("Espacio: {name}"));
            }
        }
    }

    pub(super) fn create_tab_group(&mut self, tab_id: u64, name: String, color: String) {
        let Some(ws_id) = self
            .tabs
            .iter()
            .find(|t| t.id == tab_id)
            .map(|t| t.workspace_id.clone())
        else {
            return;
        };
        let clean_name = if name.trim().is_empty() {
            format!("Grupo {}", self.tab_groups.len() + 1)
        } else {
            name.trim().chars().take(28).collect()
        };
        let clean_color = if color.trim().is_empty() {
            "#64d2ff".to_string()
        } else {
            color.trim().to_string()
        };
        let gid = generate_id("grp");
        self.tab_groups.push(TabGroup {
            id: gid.clone(),
            name: clean_name.clone(),
            color: clean_color,
            collapsed: false,
            workspace_id: ws_id,
        });
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.pinned = false;
            tab.group_id = Some(gid);
        }
        self.context_tab_id = None;
        self.prune_empty_tab_groups();
        self.mark_session_dirty();
        self.relayout();
        self.show_toast(format!("Grupo «{clean_name}» creado"));
    }

    pub(super) fn assign_tab_to_group(&mut self, tab_id: u64, group_id: Option<String>) {
        if let Some(ref gid) = group_id
            && !self.tab_groups.iter().any(|g| g.id == *gid)
        {
            return;
        }
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            if group_id.is_some() {
                tab.pinned = false;
            }
            tab.group_id = group_id;
        }
        self.context_tab_id = None;
        self.prune_empty_tab_groups();
        self.mark_session_dirty();
        self.relayout();
        self.ui_dirty = true;
    }

    pub(super) fn toggle_tab_group_collapse(&mut self, group_id: &str) {
        if let Some(g) = self.tab_groups.iter_mut().find(|g| g.id == group_id) {
            g.collapsed = !g.collapsed;
            self.mark_session_dirty();
            self.ui_dirty = true;
        }
    }

    pub(super) fn rename_tab_group(&mut self, group_id: &str, name: String, color: Option<String>) {
        if let Some(g) = self.tab_groups.iter_mut().find(|g| g.id == group_id) {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                g.name = trimmed.chars().take(28).collect();
            }
            if let Some(c) = color
                && !c.trim().is_empty()
            {
                g.color = c.trim().to_string();
            }
            self.mark_session_dirty();
            self.ui_dirty = true;
        }
    }

    pub(super) fn ungroup_tab_group(&mut self, group_id: &str) {
        for tab in &mut self.tabs {
            if tab.group_id.as_deref() == Some(group_id) {
                tab.group_id = None;
            }
        }
        self.tab_groups.retain(|g| g.id != group_id);
        self.context_tab_id = None;
        self.mark_session_dirty();
        self.ui_dirty = true;
    }

    pub(super) fn close_tab_group(&mut self, group_id: &str) {
        let ids: Vec<u64> = self
            .tabs
            .iter()
            .filter(|t| t.group_id.as_deref() == Some(group_id))
            .map(|t| t.id)
            .collect();
        self.tab_groups.retain(|g| g.id != group_id);
        self.context_tab_id = None;
        for id in ids {
            self.close_tab(id);
        }
    }

    pub(super) fn prune_empty_tab_groups(&mut self) {
        let tabs = &self.tabs;
        self.tab_groups.retain(|g| {
            tabs.iter()
                .any(|t| t.group_id.as_deref() == Some(g.id.as_str()))
        });
    }

    pub(super) fn hibernate_all_inactive(&mut self) {
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
            })
            .map(|t| t.id)
            .collect();
        let count = candidates.len();
        for id in candidates {
            self.hibernate_tab(id, true);
        }
        if count > 0 {
            self.show_toast(format!("Hibernadas {count} pestañas · RAM liberada"));
            self.refresh_memory_stats_async();
        } else {
            self.show_toast("No hay pestañas inactivas para hibernar");
        }
        self.ui_dirty = true;
    }
}
