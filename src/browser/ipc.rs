use super::*;

impl BrowserApp {
    pub(super) fn handle_context_action(&mut self, action: ContextMenuAction) {
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
            ContextMenuAction::TranslateSelection(text) => {
                let encoded = percent_encode_query(&text);
                let target = format!("https://translate.google.com/?sl=auto&tl=es&text={encoded}");
                let is_priv = self.active_tab().map(|t| t.is_private).unwrap_or(false);
                let _ = self.create_tab(&target, true, is_priv);
                self.show_toast("Traduciendo selección al español...");
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

    pub(super) fn handle_shortcut(&mut self, action: ShortcutAction) {
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
            ShortcutAction::MoveActiveTabToNewWindow => {
                let id = self.active_tab_id;
                let url = self
                    .tabs
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| t.url.clone())
                    .filter(|u| !is_internal_url(u));
                if spawn_new_browser_window(url.as_deref()) {
                    if self.tabs.len() > 1 {
                        self.close_tab(id);
                    }
                    self.show_toast("Pestaña movida a nueva ventana");
                }
            }
            ShortcutAction::OpenTaskManager => {
                self.open_panel_to(PanelTab::Tasks);
                self.refresh_memory_stats_async();
            }
            ShortcutAction::SwitchWorkspaceIndex(idx) => self.switch_workspace_by_index(idx),
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

    pub(super) fn handle_ipc(&mut self, cmd: IpcCommand) {
        match cmd {
            IpcCommand::UiReady => self.ui_dirty = true,
            IpcCommand::Navigate { input } => {
                self.close_overlay();
                let target = resolve_omnibox_input_with_custom(
                    &input,
                    self.storage.settings.search_engine,
                    &self.storage.settings.custom_search_engines,
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
                if self.overlay_mode == OverlayMode::Suggestions {
                    self.close_overlay();
                }
            }
            IpcCommand::SelectSuggestion { index } => {
                self.execute_suggestion(index);
            }
            IpcCommand::ToggleSiteInfoPopover { left } => {
                self.toggle_site_info_popover(left);
            }
            IpcCommand::CloseOverlay => {
                self.close_overlay();
            }
            IpcCommand::CreateTabGroup {
                tab_id,
                name,
                color,
            } => {
                self.create_tab_group(tab_id, name, color);
            }
            IpcCommand::AssignTabToGroup { tab_id, group_id } => {
                self.assign_tab_to_group(tab_id, group_id);
            }
            IpcCommand::ToggleTabGroupCollapse { group_id } => {
                self.toggle_tab_group_collapse(&group_id);
            }
            IpcCommand::RenameTabGroup {
                group_id,
                name,
                color,
            } => {
                self.rename_tab_group(&group_id, name, color);
            }
            IpcCommand::UngroupTabGroup { group_id } => {
                self.ungroup_tab_group(&group_id);
            }
            IpcCommand::CloseTabGroup { group_id } => {
                self.close_tab_group(&group_id);
            }
            IpcCommand::HibernateAllInactive => {
                self.hibernate_all_inactive();
            }
            IpcCommand::RefreshMemoryStats => {
                self.refresh_memory_stats_async();
            }
            IpcCommand::AddCustomSearchEngine {
                keyword,
                name,
                url_template,
            } => match self
                .storage
                .add_custom_search_engine(&keyword, &name, &url_template)
            {
                Ok(()) => {
                    self.show_toast(format!(
                        "Atajo de búsqueda !{} guardado",
                        keyword.trim().trim_start_matches('!')
                    ));
                    self.ui_dirty = true;
                }
                Err(e) => self.show_toast(e),
            },
            IpcCommand::RemoveCustomSearchEngine { keyword } => {
                self.storage.remove_custom_search_engine(&keyword);
                self.show_toast("Atajo de búsqueda eliminado");
                self.ui_dirty = true;
            }
            IpcCommand::CompleteOnboarding => {
                self.complete_onboarding();
            }
            IpcCommand::SaveGistSyncConfig { token, gist_id } => {
                let tok_trimmed = token.trim();
                let protected = if tok_trimmed.is_empty() {
                    None
                } else {
                    match protect_token_dpapi(tok_trimmed) {
                        Ok(blob) => Some(blob),
                        Err(e) => {
                            self.show_toast(e);
                            return;
                        }
                    }
                };
                self.storage.set_gist_sync_config(protected, Some(gist_id));
                self.sync_status =
                    Some("Configuración guardada (token cifrado con Windows DPAPI)".into());
                self.show_toast("Configuración de Sync E2EE guardada con Windows DPAPI");
                self.ui_dirty = true;
            }
            IpcCommand::ClearGistSyncConfig => {
                self.storage.clear_gist_sync_config();
                self.sync_status = Some("Sincronización Gist desconectada".into());
                self.show_toast("Token y configuración de Sync eliminados");
                self.ui_dirty = true;
            }
            IpcCommand::SyncGistPush { passphrase } => {
                self.start_gist_sync_push(passphrase);
            }
            IpcCommand::SyncGistPull { passphrase } => {
                self.start_gist_sync_pull(passphrase);
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
                if panel == PanelTab::Tasks {
                    self.refresh_memory_stats_async();
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
}
