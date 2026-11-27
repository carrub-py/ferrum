use super::*;

impl BrowserApp {
    pub(super) fn handle_omnibox_query(&mut self, query: String, left: f64, width: f64) {
        self.overlay_query = query.clone();
        self.overlay_rect = (left, width);

        if self.is_fullscreen || self.is_pwa_mode || query.trim().is_empty() {
            if self.overlay_mode == OverlayMode::Suggestions {
                self.close_overlay();
            }
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
            &self.storage.settings.custom_search_engines,
            &[],
        );

        if suggestions.is_empty() {
            self.close_overlay();
            return;
        }

        self.overlay_mode = OverlayMode::Suggestions;
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

    pub(super) fn toggle_site_info_popover(&mut self, left: f64) {
        if self.is_fullscreen || self.is_pwa_mode {
            return;
        }
        if self.overlay_visible && self.overlay_mode == OverlayMode::SiteInfo {
            self.close_overlay();
            return;
        }
        if self.build_site_info_snapshot().is_none() {
            self.toggle_panel(PanelTab::Shield);
            return;
        }
        self.overlay_mode = OverlayMode::SiteInfo;
        self.overlay_rect = (left.max(8.0), 320.0);
        self.overlay_suggestions.clear();
        self.overlay_visible = true;
        self.position_and_push_overlay();
    }

    pub(super) fn build_site_info_snapshot(&self) -> Option<SiteInfoSnapshot> {
        let tab = self.active_tab()?;
        if is_internal_url(&tab.url) {
            return None;
        }
        let host = extract_host(&tab.url)?;
        let (shield_enabled, site_allowlisted, tab_blocked) = self
            .adblock
            .read()
            .map(|e| {
                (
                    e.is_enabled(),
                    e.is_site_allowlisted(&host),
                    e.tab_blocked_count(tab.id),
                )
            })
            .unwrap_or((true, false, 0));
        Some(SiteInfoSnapshot {
            host,
            is_https: tab.url.starts_with("https://"),
            is_private: tab.is_private,
            shield_enabled,
            site_allowlisted,
            tab_blocked,
            zoom_percent: (tab.zoom_factor * 100.0).round() as u32,
            reader_active: tab.reader_active,
            force_dark_active: tab.force_dark_active,
            permissions: self.storage.settings.permissions.clone(),
        })
    }

    pub(super) fn handle_omnibox_key(&mut self, key: &str) {
        match key {
            "ArrowDown" => {
                if self.overlay_mode == OverlayMode::Suggestions
                    && !self.overlay_suggestions.is_empty()
                {
                    self.overlay_selected =
                        (self.overlay_selected + 1) % self.overlay_suggestions.len();
                    self.push_overlay_state();
                }
            }
            "ArrowUp" => {
                if self.overlay_mode == OverlayMode::Suggestions
                    && !self.overlay_suggestions.is_empty()
                {
                    let len = self.overlay_suggestions.len();
                    self.overlay_selected = (self.overlay_selected + len - 1) % len;
                    self.push_overlay_state();
                }
            }
            "Enter" => {
                if self.overlay_mode == OverlayMode::Suggestions
                    && !self.overlay_suggestions.is_empty()
                {
                    self.execute_suggestion(self.overlay_selected);
                } else if !self.overlay_query.trim().is_empty() {
                    let q = self.overlay_query.clone();
                    self.close_overlay();
                    let target = resolve_omnibox_input_with_custom(
                        &q,
                        self.storage.settings.search_engine,
                        &self.storage.settings.custom_search_engines,
                        self.storage.settings.https_only,
                        self.storage.settings.strip_tracking_params,
                    );
                    self.navigate_active_tab(&target);
                }
            }
            _ => {}
        }
    }

    pub(super) fn execute_suggestion(&mut self, index: usize) {
        let Some(item) = self.overlay_suggestions.get(index).cloned() else {
            self.close_overlay();
            return;
        };
        self.close_overlay();
        if item.kind == SuggestionKind::Calc {
            if webview_hooks::copy_text_to_clipboard(&item.value) {
                self.show_toast(format!("Resultado copiado: {}", item.value));
            }
            return;
        }
        if item.kind == SuggestionKind::SwitchTab
            && let Some(tab_id) = item.tab_id
        {
            self.switch_tab(tab_id);
            return;
        }
        if item.kind == SuggestionKind::Bang {
            self.navigate_active_tab(&item.value);
            return;
        }
        let target = resolve_omnibox_input_with_custom(
            &item.value,
            self.storage.settings.search_engine,
            &self.storage.settings.custom_search_engines,
            self.storage.settings.https_only,
            self.storage.settings.strip_tracking_params,
        );
        self.navigate_active_tab(&target);
    }

    pub(super) fn close_overlay(&mut self) {
        if self.overlay_visible {
            self.overlay_visible = false;
            self.overlay_mode = OverlayMode::Suggestions;
            self.overlay_suggestions.clear();
            self.overlay_selected = 0;
            webview_hooks::hide_webview_smooth(&self.overlay_webview);
            #[cfg(target_os = "windows")]
            let _ = self
                .overlay_webview
                .set_memory_usage_level(MemoryUsageLevel::Low);
        }
    }

    pub(super) fn position_and_push_overlay(&self) {
        let (win_w, win_h) = logical_window_size(&self.window);
        let toolbar_h = match self.storage.settings.layout_mode {
            LayoutMode::Minimal => TOOLBAR_HEIGHT_MINIMAL,
            LayoutMode::Compact => TOOLBAR_HEIGHT_COMPACT,
            LayoutMode::Vertical => TOOLBAR_HEIGHT_VERTICAL,
        };
        let top_y = toolbar_h.saturating_sub(3);

        let (x, w, h) = match self.overlay_mode {
            OverlayMode::Suggestions => {
                let raw_w = (self.overlay_rect.1.round() as u32).max(320);
                let x =
                    (self.overlay_rect.0.round().max(8.0) as u32).min(win_w.saturating_sub(280));
                let w = raw_w.min(win_w.saturating_sub(x + 8)).max(240);
                let rows = self.overlay_suggestions.len() as u32;
                let h = (rows * 38 + 12).min(win_h.saturating_sub(top_y + 20));
                (x, w, h)
            }
            OverlayMode::SiteInfo => {
                let w = 316u32.min(win_w.saturating_sub(16));
                let x =
                    (self.overlay_rect.0.round().max(8.0) as u32).min(win_w.saturating_sub(w + 8));
                let h = 376u32.min(win_h.saturating_sub(top_y + 16));
                (x, w, h)
            }
        };

        webview_hooks::set_webview_bounds_sync(
            &self.overlay_webview,
            Rect {
                position: LogicalPosition::new(x, top_y).into(),
                size: LogicalSize::new(w, h).into(),
            },
            self.window.scale_factor(),
        );
        #[cfg(target_os = "windows")]
        let _ = self
            .overlay_webview
            .set_memory_usage_level(MemoryUsageLevel::Normal);
        webview_hooks::bring_webview_to_top(&self.overlay_webview);
        webview_hooks::show_webview_smooth(&self.overlay_webview);
        self.push_overlay_state();
    }

    pub(super) fn push_overlay_state(&self) {
        let payload = match self.overlay_mode {
            OverlayMode::Suggestions => OverlayState {
                mode: "suggestions".into(),
                suggestions: self.overlay_suggestions.clone(),
                selected_index: self.overlay_selected,
                site_info: None,
            },
            OverlayMode::SiteInfo => OverlayState {
                mode: "site_info".into(),
                suggestions: Vec::new(),
                selected_index: 0,
                site_info: self.build_site_info_snapshot(),
            },
        };
        if let Ok(json) = serde_json::to_string(&payload) {
            let js =
                format!("window.__ferrumUpdateOverlay && window.__ferrumUpdateOverlay({json});");
            let _ = self.overlay_webview.evaluate_script(&js);
        }
    }

    pub(super) fn update_window_title(&self) {
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

    pub(super) fn save_session_now(&self) {
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
                group_id: tab.group_id.clone(),
            });
        }
        if !saved_tabs.is_empty() {
            self.storage.save_session(&SessionData {
                tabs: saved_tabs,
                active_index,
                active_workspace_id: self.active_workspace_id.clone(),
                groups: self.tab_groups.clone(),
            });
        }
    }

    pub(super) fn refresh_newtab_snapshot(&self, is_private: bool) {
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
            snap.show_onboarding = !self.storage.settings.onboarding_completed
                && !self.ephemeral_test
                && !self.is_pwa_mode
                && !is_private;
            snap.layout_mode = self.storage.settings.layout_mode;
            snap.windows_registered = self.windows_registered;
            snap.detected_browsers = self.detected_browsers.clone();
            snap.recovered_from_crash = self.recovered_from_crash;
        }
    }

    pub(super) fn persist_blocked_counter_if_needed(&mut self, force: bool) {
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

    pub(super) fn build_tab_summaries_light(&self) -> Vec<TabSummary> {
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
                    group_id: tab.group_id.clone(),
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

    pub(super) fn build_ui_state(&self, toast: Option<String>) -> UiState {
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
                    group_id: tab.group_id.clone(),
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
            tab_groups: self.tab_groups.clone(),
            custom_search_engines: self.storage.settings.custom_search_engines.clone(),
            memory: self.memory_stats.clone(),
            sync: SyncSnapshot {
                has_token: self.storage.settings.gist_token_dpapi.is_some(),
                gist_id: self.storage.settings.gist_sync_id.clone(),
                last_sync: self.storage.settings.gist_last_sync,
                busy: self.sync_busy,
                status_text: self.sync_status.clone(),
            },
            crash_reports_count: self.crash_reports_count,
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
