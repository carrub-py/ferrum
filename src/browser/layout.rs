use super::*;

impl BrowserApp {
    pub(super) fn current_chrome_height(&self) -> u32 {
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

    pub(super) fn effective_vertical_collapsed(&self, win_w: u32) -> bool {
        self.storage.settings.vertical_collapsed || win_w < 860
    }

    pub(super) fn current_left_sidebar_width(&self, win_w: u32) -> u32 {
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

    pub(super) fn current_panel_width(&self, win_w: u32, left_w: u32) -> u32 {
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

    pub(super) fn content_area_width(&self) -> u32 {
        let (win_w, _) = logical_window_size(&self.window);
        let left_w = self.current_left_sidebar_width(win_w);
        let panel_w = self.current_panel_width(win_w, left_w);
        win_w.saturating_sub(left_w + panel_w)
    }

    /// Computes exact physical-pixel rectangles (x, y, w, h) for primary content,
    /// optional split divider, and optional secondary content with zero DPI rounding gaps.
    #[allow(clippy::type_complexity)]
    pub(super) fn split_layout_physical(
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

    pub(super) fn primary_content_bounds(&self) -> Rect {
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

    pub(super) fn apply_tab_visibility_physical(&mut self, phys_w: i32, phys_h: i32, scale: f64) {
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
                if tab.is_suspended {
                    webview_hooks::resume_webview(wv);
                    tab.is_suspended = false;
                }
                webview_hooks::set_webview_bounds_physical(wv, x, y, w, h);
                webview_hooks::bring_webview_to_top(wv);
                webview_hooks::show_webview_smooth(wv);
                #[cfg(target_os = "windows")]
                let _ = wv.set_memory_usage_level(MemoryUsageLevel::Normal);
                tab.is_visible = true;
            } else if let (true, Some((x, y, w, h))) = (is_secondary, secondary_rect) {
                if tab.is_suspended {
                    webview_hooks::resume_webview(wv);
                    tab.is_suspended = false;
                }
                webview_hooks::set_webview_bounds_physical(wv, x, y, w, h);
                webview_hooks::bring_webview_to_top(wv);
                webview_hooks::show_webview_smooth(wv);
                #[cfg(target_os = "windows")]
                let _ = wv.set_memory_usage_level(MemoryUsageLevel::Normal);
                tab.is_visible = true;
            }
        }

        if let Some((x, y, w, h)) = divider_rect {
            if self.divider_webview.is_none()
                && let Ok(div) = build_internal_ui_webview(
                    &self.window,
                    #[cfg(target_os = "windows")]
                    &self.shared_env,
                    self.proxy.clone(),
                    "ferrum://localhost/divider",
                    DIVIDER_HTML,
                    Rect {
                        position: wry::dpi::PhysicalPosition::new(x, y).into(),
                        size: wry::dpi::PhysicalSize::new(w.max(1) as u32, h.max(1) as u32).into(),
                    },
                    (5, 5, 7, 255),
                )
            {
                self.divider_webview = Some(div);
            }
            if let Some(div) = &self.divider_webview {
                webview_hooks::set_webview_bounds_physical(div, x, y, w, h);
                webview_hooks::bring_webview_to_top(div);
                webview_hooks::show_webview_smooth(div);
            }
        }

        // PASS 2: Only AFTER the active view covers the viewport, hide previously visible inactive tabs
        if divider_rect.is_none()
            && let Some(div) = self.divider_webview.take()
        {
            webview_hooks::destroy_webview_smooth(div);
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

    pub(super) fn apply_tab_visibility(&mut self) {
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

    pub(super) fn relayout(&mut self) {
        self.relayout_with_physical(None);
    }

    pub(super) fn relayout_with_physical(&mut self, phys_override: Option<(u32, u32)>) {
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
                if self.vertical_webview.is_none()
                    && let Ok(vw) = build_internal_ui_webview(
                        &self.window,
                        #[cfg(target_os = "windows")]
                        &self.shared_env,
                        self.proxy.clone(),
                        "ferrum://localhost/vertical",
                        VERTICAL_HTML,
                        Rect {
                            position: wry::dpi::PhysicalPosition::new(0, top_px).into(),
                            size: wry::dpi::PhysicalSize::new(
                                left_px.max(1) as u32,
                                body_h_px.max(1) as u32,
                            )
                            .into(),
                        },
                        (5, 5, 7, 255),
                    )
                {
                    self.vertical_webview = Some(vw);
                    self.ui_dirty = true;
                }
                if let Some(vw) = &self.vertical_webview {
                    webview_hooks::set_webview_bounds_physical(vw, 0, top_px, left_px, body_h_px);
                    webview_hooks::show_webview_smooth(vw);
                }
            } else if let Some(vw) = self.vertical_webview.take() {
                webview_hooks::destroy_webview_smooth(vw);
            }

            if panel_px > 0 {
                webview_hooks::set_webview_bounds_physical(
                    &self.panel_webview,
                    phys_w - panel_px,
                    top_px,
                    panel_px,
                    body_h_px,
                );
                #[cfg(target_os = "windows")]
                let _ = self
                    .panel_webview
                    .set_memory_usage_level(MemoryUsageLevel::Normal);
                webview_hooks::show_webview_smooth(&self.panel_webview);
            } else {
                webview_hooks::hide_webview_smooth(&self.panel_webview);
                #[cfg(target_os = "windows")]
                let _ = self
                    .panel_webview
                    .set_memory_usage_level(MemoryUsageLevel::Low);
            }
        }

        self.apply_tab_visibility_physical(phys_w, phys_h, scale);

        if self.overlay_visible && !self.is_fullscreen {
            self.position_and_push_overlay();
        }
    }
}
