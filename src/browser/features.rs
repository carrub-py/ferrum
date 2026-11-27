use super::*;

impl BrowserApp {
    pub(super) fn start_update_check(&mut self, silent: bool) {
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

    pub(super) fn start_update_install(&mut self) {
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

    pub(super) fn save_pending_login_if_unlocked(&mut self) {
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

    pub(super) fn quick_autofill(&mut self) {
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

    pub(super) fn inject_autofill(&self, user: &str, pass: &str) {
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

    pub(super) fn start_element_picker(&mut self) {
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

    pub(super) fn apply_user_cosmetic_on_active_tab(&self) {
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

    pub(super) fn any_audio_tab(&self) -> bool {
        self.tabs.iter().any(|t| t.is_playing_audio || t.is_muted)
    }

    pub(super) fn navigate_active_tab(&mut self, url: &str) {
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
                if tab.is_suspended {
                    webview_hooks::resume_webview(wv);
                    tab.is_suspended = false;
                }
                let _ = wv.load_url(url);
            }
        }
        self.ui_dirty = true;
    }

    pub(super) fn adjust_zoom(&mut self, delta: f64) {
        let next = self
            .active_tab()
            .map(|t| ((t.zoom_factor + delta) * 10.0).round() / 10.0)
            .unwrap_or(1.0);
        self.set_zoom(next);
    }

    pub(super) fn set_zoom(&mut self, factor: f64) {
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

    pub(super) fn open_find(&mut self) {
        self.close_overlay();
        self.find.open = true;
        self.relayout();
        self.ui_dirty = true;
        let _ = self.chrome_webview.focus();
        let _ = self.chrome_webview.evaluate_script(
            "setTimeout(() => window.__ferrumFocusFind && window.__ferrumFocusFind(), 20);",
        );
    }

    pub(super) fn close_find(&mut self) {
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

    pub(super) fn run_find(&mut self, query: String, case_sensitive: bool, step: i32) {
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

    pub(super) fn toggle_reader_mode(&self) {
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

    pub(super) fn toggle_force_dark(&self) {
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

    pub(super) fn toggle_pip(&self) {
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

    pub(super) fn capture_screenshot(&self) {
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

    pub(super) fn copy_clean_url(&mut self) {
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

    pub(super) fn translate_current_page(&mut self) {
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

    pub(super) fn set_fullscreen(&mut self, fs: bool) {
        if self.is_fullscreen == fs {
            return;
        }
        self.close_overlay();
        self.is_fullscreen = fs;
        if fs {
            self.window
                .set_fullscreen(Some(Fullscreen::Borderless(None)));
            webview_hooks::hide_webview_smooth(&self.chrome_webview);
            if let Some(vw) = &self.vertical_webview {
                webview_hooks::hide_webview_smooth(vw);
            }
            if let Some(div) = &self.divider_webview {
                webview_hooks::hide_webview_smooth(div);
            }
            webview_hooks::hide_webview_smooth(&self.panel_webview);
        } else {
            self.window.set_fullscreen(None);
        }
        self.relayout();
        self.ui_dirty = true;
    }

    pub(super) fn reload_active_if_web(&self) {
        if let Some(tab) = self.active_tab()
            && !is_internal_url(&tab.url)
            && let Some(wv) = &tab.webview
        {
            let _ = wv.reload();
        }
    }

    pub(super) fn sync_page_shield_scripts_on_active_tab(&self) {
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

    pub(super) fn open_panel_to(&mut self, panel: PanelTab) {
        if self.is_pwa_mode {
            return;
        }
        self.close_overlay();
        self.open_panel = Some(panel);
        self.relayout();
        self.ui_dirty = true;
    }

    pub(super) fn toggle_panel(&mut self, panel: PanelTab) {
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

    pub(super) fn toggle_bookmark_current(&mut self) {
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

    pub(super) fn focus_omnibox(&self) {
        if self.is_pwa_mode {
            return;
        }
        let _ = self.chrome_webview.focus();
        let _ = self
            .chrome_webview
            .evaluate_script("window.__ferrumFocusOmnibox && window.__ferrumFocusOmnibox();");
    }

    pub(super) fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == self.active_tab_id)
    }

    pub(super) fn active_webview(&self) -> Option<&WebView> {
        self.active_tab().and_then(|t| t.webview.as_ref())
    }

    pub(super) fn refresh_memory_stats_async(&self) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let stats = sample_browser_memory();
            let _ = proxy.send_event(BrowserEvent::MemoryStatsReady(stats));
        });
    }

    pub(super) fn complete_onboarding(&mut self) {
        self.storage.mark_onboarding_completed();
        self.refresh_newtab_snapshot(false);
        if let Some(tab) = self.active_tab()
            && is_internal_url(&tab.url)
            && let Some(wv) = &tab.webview
        {
            let _ = wv.load_url(INTERNAL_NEWTAB_URL);
        }
        self.ui_dirty = true;
    }

    pub(super) fn start_gist_sync_push(&mut self, passphrase: String) {
        if self.sync_busy {
            return;
        }
        let Some(token_blob) = self.storage.settings.gist_token_dpapi.clone() else {
            self.show_toast("Guarda primero tu token de GitHub (scope gist)");
            return;
        };
        let token = match unprotect_token_dpapi(&token_blob) {
            Ok(t) if !t.trim().is_empty() => t,
            Ok(_) => {
                self.show_toast("Token de GitHub vacío");
                return;
            }
            Err(e) => {
                self.show_toast(e);
                return;
            }
        };
        self.sync_busy = true;
        self.sync_status = Some("Cifrando localmente y subiendo a Gist privado...".into());
        self.ui_dirty = true;

        let data_dir = self.storage.data_dir();
        let existing_id = self.storage.settings.gist_sync_id.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let res = push_encrypted_gist(&data_dir, &token, existing_id.as_deref(), &passphrase);
            let _ = proxy.send_event(BrowserEvent::GistSyncPushFinished(res));
        });
    }

    pub(super) fn start_gist_sync_pull(&mut self, passphrase: String) {
        if self.sync_busy {
            return;
        }
        let Some(token_blob) = self.storage.settings.gist_token_dpapi.clone() else {
            self.show_toast("Guarda primero tu token de GitHub (scope gist)");
            return;
        };
        let Some(gist_id) = self.storage.settings.gist_sync_id.clone() else {
            self.show_toast("Indica el ID del Gist privado para descargar");
            return;
        };
        let token = match unprotect_token_dpapi(&token_blob) {
            Ok(t) if !t.trim().is_empty() => t,
            Ok(_) => {
                self.show_toast("Token de GitHub vacío");
                return;
            }
            Err(e) => {
                self.show_toast(e);
                return;
            }
        };
        self.sync_busy = true;
        self.sync_status = Some("Descargando y descifrando desde Gist privado...".into());
        self.ui_dirty = true;

        let data_dir = self.storage.data_dir();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let res = pull_encrypted_gist(&data_dir, &token, &gist_id, &passphrase);
            let _ = proxy.send_event(BrowserEvent::GistSyncPullFinished(res));
        });
    }
}
