use adblock::Engine as BraveAdblockEngine;
use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request as AdblockRequest;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use url::Url;

use crate::navigation::is_internal_url;
use crate::storage::{BrowserSettings, unix_now};

const MAX_RECENT_BLOCKED: usize = 80;
const FILTER_DAT_FILENAME: &str = "ferrum_lists.dat";

const COMMUNITY_FILTER_LISTS: &[(&str, &str)] = &[
    ("EasyList", "https://easylist.to/easylist/easylist.txt"),
    (
        "EasyPrivacy",
        "https://easylist.to/easylist/easyprivacy.txt",
    ),
    (
        "Peter Lowe",
        "https://pgl.yoyo.org/adservers/serverlist.php?hostformat=adblockplus&showintro=0&mimetype=plaintext",
    ),
    (
        "Fanboy Cookies",
        "https://secure.fanboy.co.nz/fanboy-cookiemonster.txt",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockCategory {
    Ad,
    Tracker,
    Analytics,
    Social,
    Security,
    EasyList,
    Custom,
}

impl BlockCategory {
    pub fn label_es(&self) -> &'static str {
        match self {
            Self::Ad => "Publicidad",
            Self::Tracker => "Rastreador",
            Self::Analytics => "Analítica",
            Self::Social => "Social",
            Self::Security => "Malware/Minero",
            Self::EasyList => "EasyList",
            Self::Custom => "Personalizado",
        }
    }

    pub fn estimated_bytes_saved(&self) -> u64 {
        match self {
            Self::Ad | Self::EasyList => 42_000,
            Self::Tracker => 6_500,
            Self::Analytics => 19_000,
            Self::Social => 24_000,
            Self::Security => 65_000,
            Self::Custom => 22_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedEvent {
    pub tab_id: u64,
    pub domain: String,
    pub url: String,
    pub category: BlockCategory,
    pub category_label: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct BlockDecision {
    pub domain: String,
    pub category: BlockCategory,
}

pub struct AdBlockEngine {
    enabled: bool,
    cosmetic_enabled: bool,
    block_cookie_banners: bool,
    anti_fingerprint: bool,
    builtin_domains: HashMap<&'static str, BlockCategory>,
    custom_blocked: HashSet<String>,
    allowlist: HashSet<String>,
    brave_engine: Option<BraveAdblockEngine>,
    community_rules_count: usize,
    total_blocked: AtomicU64,
    bytes_saved: AtomicU64,
    tab_counts: HashMap<u64, u64>,
    tab_recent: HashMap<u64, VecDeque<BlockedEvent>>,
}

impl AdBlockEngine {
    pub fn new(settings: &BrowserSettings, filters_dir: &Path) -> Self {
        let mut builtin_domains = HashMap::with_capacity(380);

        for &d in AD_DOMAINS {
            builtin_domains.insert(d, BlockCategory::Ad);
        }
        for &d in TRACKER_DOMAINS {
            builtin_domains.insert(d, BlockCategory::Tracker);
        }
        for &d in ANALYTICS_DOMAINS {
            builtin_domains.insert(d, BlockCategory::Analytics);
        }
        for &d in SOCIAL_TRACKER_DOMAINS {
            builtin_domains.insert(d, BlockCategory::Social);
        }
        for &d in SECURITY_MALWARE_DOMAINS {
            builtin_domains.insert(d, BlockCategory::Security);
        }

        let custom_blocked = settings
            .custom_blocked_domains
            .iter()
            .map(|d| normalize_domain(d))
            .filter(|d| !d.is_empty())
            .collect();

        let allowlist = settings
            .allowlisted_domains
            .iter()
            .map(|d| normalize_domain(d))
            .filter(|d| !d.is_empty())
            .collect();

        // Fast-load cached binary FlatBuffer EasyList/EasyPrivacy snapshot if previously downloaded
        let mut brave_engine = None;
        let mut community_rules_count = 0;
        let dat_path = filters_dir.join(FILTER_DAT_FILENAME);
        if let Ok(bytes) = fs::read(&dat_path) {
            let mut engine = BraveAdblockEngine::default();
            if engine.deserialize(&bytes).is_ok() {
                brave_engine = Some(engine);
                community_rules_count = settings.filter_lists_rules_count;
            }
        }

        Self {
            enabled: settings.shield_enabled,
            cosmetic_enabled: settings.cosmetic_filtering,
            block_cookie_banners: settings.block_cookie_banners,
            anti_fingerprint: settings.anti_fingerprint,
            builtin_domains,
            custom_blocked,
            allowlist,
            brave_engine,
            community_rules_count,
            total_blocked: AtomicU64::new(settings.total_blocked_lifetime),
            bytes_saved: AtomicU64::new(settings.bytes_saved_lifetime),
            tab_counts: HashMap::new(),
            tab_recent: HashMap::new(),
        }
    }

    pub fn install_brave_engine(&mut self, engine: BraveAdblockEngine, rules_count: usize) {
        self.brave_engine = Some(engine);
        self.community_rules_count = rules_count;
    }

    pub fn has_community_lists(&self) -> bool {
        self.brave_engine.is_some()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn is_cosmetic_enabled(&self) -> bool {
        self.cosmetic_enabled
    }

    pub fn set_cosmetic_enabled(&mut self, enabled: bool) {
        self.cosmetic_enabled = enabled;
    }

    pub fn is_cookie_banners_blocked(&self) -> bool {
        self.block_cookie_banners
    }

    pub fn set_cookie_banners_blocked(&mut self, enabled: bool) {
        self.block_cookie_banners = enabled;
    }

    pub fn is_anti_fingerprint(&self) -> bool {
        self.anti_fingerprint
    }

    pub fn set_anti_fingerprint(&mut self, enabled: bool) {
        self.anti_fingerprint = enabled;
    }

    pub fn total_blocked(&self) -> u64 {
        self.total_blocked.load(Ordering::Relaxed)
    }

    pub fn bytes_saved(&self) -> u64 {
        self.bytes_saved.load(Ordering::Relaxed)
    }

    pub fn rules_count(&self) -> usize {
        self.builtin_domains.len()
            + self.custom_blocked.len()
            + URL_SUBSTRING_RULES.len()
            + self.community_rules_count
    }

    pub fn is_site_allowlisted(&self, host: &str) -> bool {
        let norm = normalize_domain(host);
        if norm.is_empty() {
            return false;
        }
        domain_matches_set(&norm, &self.allowlist)
    }

    pub fn toggle_site_allowlist(&mut self, host: &str) -> bool {
        let norm = normalize_domain(host);
        if norm.is_empty() {
            return false;
        }
        if self.allowlist.contains(&norm) {
            self.allowlist.remove(&norm);
            false
        } else {
            self.allowlist.insert(norm);
            true
        }
    }

    pub fn remove_allowlist_domain(&mut self, domain: &str) {
        let norm = normalize_domain(domain);
        self.allowlist.remove(&norm);
    }

    pub fn add_custom_blocked_domain(&mut self, domain: &str) -> bool {
        let norm = normalize_domain(domain);
        if norm.is_empty() {
            return false;
        }
        self.custom_blocked.insert(norm)
    }

    pub fn remove_custom_blocked_domain(&mut self, domain: &str) {
        let norm = normalize_domain(domain);
        self.custom_blocked.remove(&norm);
    }

    pub fn allowlisted_domains_sorted(&self) -> Vec<String> {
        let mut v: Vec<String> = self.allowlist.iter().cloned().collect();
        v.sort();
        v
    }

    pub fn custom_blocked_sorted(&self) -> Vec<String> {
        let mut v: Vec<String> = self.custom_blocked.iter().cloned().collect();
        v.sort();
        v
    }

    pub fn tab_blocked_count(&self, tab_id: u64) -> u64 {
        self.tab_counts.get(&tab_id).copied().unwrap_or(0)
    }

    pub fn tab_recent_events(&self, tab_id: u64) -> Vec<BlockedEvent> {
        self.tab_recent
            .get(&tab_id)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn reset_tab_stats(&mut self, tab_id: u64) {
        self.tab_counts.insert(tab_id, 0);
        if let Some(q) = self.tab_recent.get_mut(&tab_id) {
            q.clear();
        }
    }

    pub fn remove_tab_stats(&mut self, tab_id: u64) {
        self.tab_counts.remove(&tab_id);
        self.tab_recent.remove(&tab_id);
    }

    pub fn record_blocked(&mut self, tab_id: u64, url: &str, decision: &BlockDecision) {
        self.total_blocked.fetch_add(1, Ordering::Relaxed);
        self.bytes_saved
            .fetch_add(decision.category.estimated_bytes_saved(), Ordering::Relaxed);
        *self.tab_counts.entry(tab_id).or_insert(0) += 1;

        let queue = self
            .tab_recent
            .entry(tab_id)
            .or_insert_with(|| VecDeque::with_capacity(MAX_RECENT_BLOCKED));

        queue.push_front(BlockedEvent {
            tab_id,
            domain: decision.domain.clone(),
            url: truncate_url(url, 180),
            category: decision.category,
            category_label: decision.category.label_es().to_string(),
            timestamp: unix_now(),
        });

        while queue.len() > MAX_RECENT_BLOCKED {
            queue.pop_back();
        }
    }

    /// Returns domain-specific cosmetic CSS rules from Brave `adblock-rust` if loaded.
    pub fn url_cosmetic_css(&self, page_url: &str, host: Option<&str>) -> Option<String> {
        if !self.enabled || !self.cosmetic_enabled {
            return None;
        }
        if let Some(h) = host
            && self.is_site_allowlisted(h)
        {
            return None;
        }
        let engine = self.brave_engine.as_ref()?;
        let res = engine.url_cosmetic_resources(page_url);
        if res.hide_selectors.is_empty() {
            return None;
        }
        let selectors: Vec<&str> = res
            .hide_selectors
            .iter()
            .take(180)
            .map(|s| s.as_str())
            .collect();
        if selectors.is_empty() {
            None
        } else {
            Some(format!(
                "{} {{ display: none !important; visibility: hidden !important; }}",
                selectors.join(", ")
            ))
        }
    }

    pub fn check_request(
        &self,
        request_url: &str,
        page_host: Option<&str>,
        resource_type: &str,
        is_main_document: bool,
    ) -> Option<BlockDecision> {
        if !self.enabled {
            return None;
        }

        if is_internal_url(request_url)
            || (!request_url.starts_with("http://") && !request_url.starts_with("https://"))
        {
            return None;
        }

        let parsed = Url::parse(request_url).ok()?;
        let req_host = parsed.host_str()?.to_ascii_lowercase();

        // Always block malware & cryptominers unless allowlisted
        if !domain_matches_set(&req_host, &self.allowlist)
            && self.match_builtin_domain(&req_host) == Some(BlockCategory::Security)
        {
            return Some(BlockDecision {
                domain: req_host,
                category: BlockCategory::Security,
            });
        }

        if let Some(site_host) = page_host
            && self.is_site_allowlisted(site_host)
        {
            return None;
        }

        if domain_matches_set(&req_host, &self.allowlist) {
            return None;
        }

        if is_main_document
            && let Some(site_host) = page_host
            && registrable_approx(&req_host) == registrable_approx(&site_host.to_ascii_lowercase())
        {
            return None;
        }

        if domain_matches_set(&req_host, &self.custom_blocked) {
            return Some(BlockDecision {
                domain: req_host,
                category: BlockCategory::Custom,
            });
        }

        // Fast built-in domain match
        if let Some(category) = self.match_builtin_domain(&req_host) {
            if category == BlockCategory::Social
                && let Some(site_host) = page_host
                && registrable_approx(&req_host)
                    == registrable_approx(&site_host.to_ascii_lowercase())
            {
                return None;
            }
            return Some(BlockDecision {
                domain: req_host,
                category,
            });
        }

        // Brave adblock-rust community engine (EasyList + EasyPrivacy + Peter Lowe + Fanboy Cookies)
        if !is_main_document && let Some(engine) = &self.brave_engine {
            let source_url = page_host
                .map(|h| format!("https://{h}/"))
                .unwrap_or_else(|| request_url.to_string());
            if let Ok(req) = AdblockRequest::new(request_url, &source_url, resource_type, "get") {
                let res = engine.check_network_request(&req);
                if res.should_block() {
                    return Some(BlockDecision {
                        domain: req_host,
                        category: BlockCategory::EasyList,
                    });
                }
            }
        }

        // URL substring rules fallback
        if !is_main_document {
            let lower_path_query = match parsed.query() {
                Some(q) => format!(
                    "{}?{}",
                    parsed.path().to_ascii_lowercase(),
                    q.to_ascii_lowercase()
                ),
                None => parsed.path().to_ascii_lowercase(),
            };
            let is_third_party = page_host
                .map(|ph| {
                    registrable_approx(&req_host) != registrable_approx(&ph.to_ascii_lowercase())
                })
                .unwrap_or(true);

            for &(pattern, category, third_party_only) in URL_SUBSTRING_RULES {
                if third_party_only && !is_third_party {
                    continue;
                }
                if lower_path_query.contains(pattern) {
                    return Some(BlockDecision {
                        domain: req_host,
                        category,
                    });
                }
            }
        }

        None
    }

    fn match_builtin_domain(&self, host: &str) -> Option<BlockCategory> {
        let mut current = host;
        loop {
            if let Some(&cat) = self.builtin_domains.get(current) {
                return Some(cat);
            }
            let idx = current.find('.')?;
            current = &current[idx + 1..];
        }
    }
}

/// Downloads EasyList, EasyPrivacy, Peter Lowe & Fanboy Cookie lists over HTTPS,
/// compiles them into an optimized Brave `adblock::Engine`, and saves a fast `.dat` cache.
pub fn download_and_compile_filter_lists(
    filters_dir: PathBuf,
) -> Result<(BraveAdblockEngine, usize, usize), String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build();
    let agent: ureq::Agent = config.into();

    let mut filter_set = FilterSet::new(false);
    let mut loaded_sources = 0usize;
    let mut approx_rules = 0usize;

    for &(_name, list_url) in COMMUNITY_FILTER_LISTS {
        let resp = agent
            .get(list_url)
            .header("User-Agent", "FerrumBrowser/0.3")
            .call();
        if let Ok(mut response) = resp
            && let Ok(text) = response.body_mut().read_to_string()
        {
            let count = text
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    !t.is_empty() && !t.starts_with('!') && !t.starts_with('[')
                })
                .count();
            if count > 50 {
                filter_set.add_filter_list(text, ParseOptions::default());
                loaded_sources += 1;
                approx_rules += count;
            }
        }
    }

    if loaded_sources == 0 {
        return Err(
            "No se pudo descargar ninguna lista de filtros. Verifica tu conexión.".to_string(),
        );
    }

    let engine = BraveAdblockEngine::new_with_filter_set(filter_set);
    let serialized = engine.serialize();
    let _ = fs::create_dir_all(&filters_dir);
    let dat_path = filters_dir.join(FILTER_DAT_FILENAME);
    let tmp = dat_path.with_extension("dat.tmp");
    if fs::write(&tmp, &serialized).is_ok() {
        let _ = fs::rename(&tmp, &dat_path).or_else(|_| fs::write(&dat_path, &serialized));
    }

    Ok((engine, approx_rules, loaded_sources))
}

pub fn normalize_domain(raw: &str) -> String {
    let s = raw.trim().to_ascii_lowercase();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(&s);
    let host = s.split('/').next().unwrap_or(s);
    let host = host.split(':').next().unwrap_or(host);
    host.trim_start_matches("www.")
        .trim_matches('.')
        .to_string()
}

fn domain_matches_set(host: &str, set: &HashSet<String>) -> bool {
    if set.is_empty() {
        return false;
    }
    let clean = host.trim_start_matches("www.");
    let mut current = clean;
    loop {
        if set.contains(current) {
            return true;
        }
        match current.find('.') {
            Some(idx) => current = &current[idx + 1..],
            None => return false,
        }
    }
}

fn registrable_approx(host: &str) -> &str {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() <= 2 {
        return host;
    }
    let last_two_start =
        host.len() - parts[parts.len() - 1].len() - parts[parts.len() - 2].len() - 1;
    &host[last_two_start..]
}

fn truncate_url(url: &str, max: usize) -> String {
    if url.len() <= max {
        url.to_string()
    } else {
        let mut s: String = url.chars().take(max.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

/// Builds the unified page initialization script with an unguessable per-session nonce
/// trapped inside a private IIFE closure so untrusted webpage scripts can NEVER forge
/// Element Picker or Login Detection messages back to Rust.
pub fn build_page_init_script(bridge_nonce: &str) -> String {
    PAGE_SHIELD_AND_TOOLS_INIT_SCRIPT.replace("__FERRUM_BRIDGE_NONCE__", bridge_nonce)
}

/// Unified initialization script injected into content tabs:
/// 1) Cosmetic ad-hiding CSS + dynamic EasyList CSS + persistent Element Picker CSS
/// 2) Automatic Cookie Consent / GDPR banner dismissal & scroll-lock rescue
/// 3) Anti-fingerprinting + GPC + anti-beacon guard
/// 4) In-page Find engine
/// 5) Smart OLED Dark Mode + Apple Focus Reader Mode
/// 6) 1-Click Picture-in-Picture (PiP) + On-Demand Vault Credential Autofill
/// 7) Visual Element Picker (uBlock-style) + Automatic Login Capture prompt
pub const PAGE_SHIELD_AND_TOOLS_INIT_SCRIPT: &str = r#"
(function() {
  if (window.__ferrumInjected) return;
  window.__ferrumInjected = true;
  if (location.hostname === 'ferrum.localhost') return;
  const __FERRUM_NONCE = "__FERRUM_BRIDGE_NONCE__";
  const postBridge = (payload) => {
    try {
      if (window.ipc && typeof window.ipc.postMessage === 'function') {
        window.ipc.postMessage(JSON.stringify(Object.assign({ __ferrum_bridge_nonce: __FERRUM_NONCE }, payload)));
      }
    } catch (_) {}
  };

  // --- 1. COSMETIC AD HIDING ---
  const COSMETIC_CSS = `
    ins.adsbygoogle,
    [id^="google_ads_iframe"],
    [id^="div-gpt-ad"],
    [data-ad-slot],
    [data-ad-unit],
    [data-google-query-id],
    .adsbygoogle,
    .ad-banner,
    .ad-container,
    .ad-wrapper,
    .ad-slot,
    .advertisement,
    .sponsored-ad,
    .sponsored-content,
    .taboola-widget,
    #taboola-below-article-thumbnails,
    .OUTBRAIN,
    [class*="outbrain"],
    [id*="taboola"],
    iframe[src*="doubleclick.net"],
    iframe[src*="googlesyndication.com"],
    iframe[src*="adnxs.com"],
    iframe[src*="criteo."],
    iframe[src*="amazon-adsystem.com"],
    ytd-ad-slot-renderer,
    ytd-display-ad-renderer,
    ytd-promoted-sparkles-web-renderer,
    #player-ads,
    .ytp-ad-overlay-container,
    .ytp-ad-text-overlay {
      display: none !important;
      visibility: hidden !important;
      height: 0 !important;
      min-height: 0 !important;
      max-height: 0 !important;
      overflow: hidden !important;
      pointer-events: none !important;
    }
  `;

  const COOKIE_BANNER_CSS = `
    #onetrust-consent-sdk, #onetrust-banner-sdk, .onetrust-pc-dark-filter,
    #didomi-host, .didomi-popup-container, .didomi-notice-popup,
    #CybotCookiebotDialog, #CybotCookiebotDialogBodyUnderlay,
    .qc-cmp2-container, #qc-cmp2-main,
    [id*="sp_message_container"], .sp-message-open,
    #usercentrics-root, #truste-consent-track,
    .fc-consent-root, .fc-dialog-overlay,
    [aria-label*="cookie" i][role="dialog"],
    [id*="cookie-banner" i], [id*="cookie-consent" i], [id*="cookieBanner" i],
    [class*="cookie-banner" i], [class*="cookie-consent" i], [class*="gdpr-banner" i] {
      display: none !important;
      visibility: hidden !important;
      opacity: 0 !important;
      pointer-events: none !important;
    }
  `;

  function applyStyle(id, css, enabled) {
    const existing = document.getElementById(id);
    if (!enabled) { if (existing) existing.remove(); return; }
    if (existing) { existing.textContent = css; return; }
    const st = document.createElement('style');
    st.id = id;
    st.textContent = css;
    (document.head || document.documentElement).appendChild(st);
  }

  window.__ferrumSetCosmetic = function(enabled) {
    window.__ferrumCosmeticDisabled = !enabled;
    applyStyle('__ferrum_cosmetic_style', COSMETIC_CSS, !!enabled);
  };

  window.__ferrumInjectExtraCosmetic = function(css) {
    if (!css || window.__ferrumCosmeticDisabled) return;
    applyStyle('__ferrum_easylist_cosmetic', css, true);
  };

  window.__ferrumSetUserCosmetic = function(css) {
    applyStyle('__ferrum_user_cosmetic', css || '', !!(css && css.trim()));
  };

  window.__ferrumSetCookieBlock = function(enabled) {
    window.__ferrumCookieBlockDisabled = !enabled;
    applyStyle('__ferrum_cookie_style', COOKIE_BANNER_CSS, !!enabled);
    if (enabled && document.body) {
      const unlock = () => {
        if (document.body.style.overflow === 'hidden' && document.querySelector('#onetrust-consent-sdk, #didomi-host, .fc-consent-root, .qc-cmp2-container')) {
          document.body.style.overflow = '';
          document.documentElement.style.overflow = '';
        }
      };
      setTimeout(unlock, 600);
    }
  };

  // --- 2. PRIVACY HARDENING (GPC + ANTI-BEACON + BATTERY FINGERPRINT GUARD) ---
  window.__ferrumAntiFpEnabled = true;
  try {
    Object.defineProperty(navigator, 'globalPrivacyControl', { get: () => true, configurable: true });
    Object.defineProperty(navigator, 'doNotTrack', { get: () => '1', configurable: true });
    if (typeof navigator.getBattery === 'function') {
      const origBattery = navigator.getBattery.bind(navigator);
      navigator.getBattery = function() {
        if (window.__ferrumAntiFpEnabled) {
          return Promise.resolve({
            charging: true, chargingTime: 0, dischargingTime: Infinity, level: 1.0,
            addEventListener() {}, removeEventListener() {}
          });
        }
        return origBattery();
      };
    }
    if (typeof navigator.sendBeacon === 'function') {
      const origBeacon = navigator.sendBeacon.bind(navigator);
      const BLOCK_RE = /(doubleclick|googlesyndication|google-analytics|googletagmanager|facebook\.com\/tr|clarity\.ms|hotjar|mixpanel|segment\.io|amplitude|datadoghq|nr-data|scorecardresearch|quantserve|\/g\/collect|\/telemetry\/)/i;
      navigator.sendBeacon = function(url, data) {
        if (window.__ferrumAntiFpEnabled && typeof url === 'string' && BLOCK_RE.test(url)) {
          return true;
        }
        return origBeacon(url, data);
      };
    }
  } catch (_) {}

  window.__ferrumSetAntiFp = function(enabled) {
    window.__ferrumAntiFpEnabled = !!enabled;
  };

  // --- 3. IN-PAGE FIND ENGINE ---
  let findMarks = [];
  let findIndex = -1;
  let lastFindKey = '';

  function ensureFindStyle() {
    if (document.getElementById('__ferrum_find_style')) return;
    const st = document.createElement('style');
    st.id = '__ferrum_find_style';
    st.textContent = `
      mark.__ferrum_find { background: rgba(255, 214, 10, 0.38) !important; color: inherit !important; border-radius: 3px; padding: 0 1px; }
      mark.__ferrum_find.__ferrum_find_active { background: #ff9f0a !important; color: #000 !important; box-shadow: 0 0 0 2px rgba(255, 159, 10, 0.45); }
    `;
    (document.head || document.documentElement).appendChild(st);
  }

  window.__ferrumFindClear = function() {
    for (const m of findMarks) {
      const p = m.parentNode;
      if (p) {
        p.replaceChild(document.createTextNode(m.textContent || ''), m);
        p.normalize();
      }
    }
    findMarks = [];
    findIndex = -1;
    lastFindKey = '';
    return JSON.stringify({ current: 0, total: 0 });
  };

  window.__ferrumFind = function(query, caseSensitive, step) {
    const q = String(query || '');
    if (!q) return window.__ferrumFindClear();
    ensureFindStyle();
    const key = (caseSensitive ? '1:' : '0:') + q;
    if (key !== lastFindKey) {
      window.__ferrumFindClear();
      lastFindKey = key;
      const needle = caseSensitive ? q : q.toLowerCase();
      const skip = new Set(['SCRIPT','STYLE','NOSCRIPT','TEXTAREA','INPUT','SELECT','IFRAME']);
      const walker = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT);
      const nodes = [];
      let n;
      while ((n = walker.nextNode())) {
        if (!n.nodeValue || !n.parentElement) continue;
        if (skip.has(n.parentElement.tagName)) continue;
        if (n.parentElement.closest('#__ferrum_reader_overlay')) continue;
        nodes.push(n);
      }
      for (const node of nodes) {
        const text = node.nodeValue;
        const hay = caseSensitive ? text : text.toLowerCase();
        if (!hay.includes(needle)) continue;
        const frag = document.createDocumentFragment();
        let pos = 0;
        let idx;
        while ((idx = hay.indexOf(needle, pos)) !== -1) {
          if (idx > pos) frag.appendChild(document.createTextNode(text.slice(pos, idx)));
          const mark = document.createElement('mark');
          mark.className = '__ferrum_find';
          mark.textContent = text.slice(idx, idx + q.length);
          frag.appendChild(mark);
          findMarks.push(mark);
          pos = idx + q.length;
        }
        if (pos < text.length) frag.appendChild(document.createTextNode(text.slice(pos)));
        node.parentNode && node.parentNode.replaceChild(frag, node);
      }
      findIndex = findMarks.length ? 0 : -1;
    } else if (findMarks.length) {
      findIndex = (findIndex + (step || 1) + findMarks.length) % findMarks.length;
    }

    findMarks.forEach((m, i) => m.classList.toggle('__ferrum_find_active', i === findIndex));
    if (findIndex >= 0 && findMarks[findIndex]) {
      findMarks[findIndex].scrollIntoView({ behavior: 'smooth', block: 'center', inline: 'nearest' });
    }
    return JSON.stringify({ current: findMarks.length ? findIndex + 1 : 0, total: findMarks.length });
  };

  // --- 4. SMART OLED DARK MODE ---
  window.__ferrumToggleForceDark = function() {
    const existing = document.getElementById('__ferrum_force_dark');
    if (existing) { existing.remove(); return 'false'; }
    const st = document.createElement('style');
    st.id = '__ferrum_force_dark';
    st.textContent = `
      html { filter: invert(0.94) hue-rotate(180deg) !important; background: #000 !important; }
      img, video, picture, canvas, svg, iframe, [style*="background-image"] {
        filter: invert(1) hue-rotate(180deg) !important;
      }
    `;
    (document.head || document.documentElement).appendChild(st);
    return 'true';
  };

  // --- 5. APPLE SAFARI FOCUS READER MODE ---
  window.__ferrumToggleReader = function() {
    const existing = document.getElementById('__ferrum_reader_overlay');
    if (existing) { existing.remove(); document.documentElement.style.overflow = ''; return 'false'; }
    const candidates = Array.from(document.querySelectorAll('article, main, [role="main"], .post-content, .article-body, .entry-content'));
    let best = null, bestScore = 0;
    for (const el of candidates) {
      const len = (el.innerText || '').trim().length;
      if (len > bestScore) { bestScore = len; best = el; }
    }
    if (!best || bestScore < 200) best = document.body;

    const blocks = [], seen = new Set();
    best.querySelectorAll('h1, h2, h3, h4, p, blockquote, pre, ul, ol').forEach(el => {
      if (el.closest('nav, header, footer, aside, form, [role="navigation"]')) return;
      const txt = (el.innerText || '').trim();
      if (txt.length < 25 && !/^H[1-4]$/.test(el.tagName)) return;
      if (seen.has(txt)) return;
      seen.add(txt);
      blocks.push({ tag: el.tagName.toLowerCase(), text: txt });
    });

    const wordCount = blocks.reduce((acc, b) => acc + b.text.split(/\s+/).length, 0);
    const readMins = Math.max(1, Math.round(wordCount / 220));
    const title = document.querySelector('h1')?.innerText?.trim() || document.title || location.hostname;

    const overlay = document.createElement('div');
    overlay.id = '__ferrum_reader_overlay';
    overlay.dataset.theme = 'obsidian';
    overlay.innerHTML = `
      <style>
        #__ferrum_reader_overlay {
          position: fixed; inset: 0; z-index: 2147483647; overflow-y: auto;
          background: #060608; color: #ececf1;
          font-family: -apple-system, BlinkMacSystemFont, "New York", Georgia, serif;
          padding: 0 24px 96px;
        }
        #__ferrum_reader_overlay[data-theme="sepia"] { background: #191611; color: #e8dcc8; }
        #__ferrum_reader_overlay[data-theme="light"] { background: #fbfbfd; color: #1d1d1f; }
        .__fr_bar {
          position: sticky; top: 0; max-width: 700px; margin: 0 auto 36px;
          padding: 14px 0; display: flex; align-items: center; justify-content: space-between;
          background: inherit; border-bottom: 1px solid rgba(128,128,128,0.18);
          font-family: -apple-system, "Segoe UI", system-ui, sans-serif; font-size: 12px;
        }
        .__fr_controls { display: flex; gap: 6px; align-items: center; }
        .__fr_btn {
          border: 1px solid rgba(128,128,128,0.24); background: rgba(128,128,128,0.1);
          color: inherit; border-radius: 999px; padding: 5px 11px; cursor: pointer;
          font-family: inherit; font-size: 12px; font-weight: 500;
        }
        .__fr_btn:hover { background: rgba(128,128,128,0.22); }
        .__fr_article { max-width: 660px; margin: 0 auto; font-size: 19px; line-height: 1.75; }
        .__fr_article h1 { font-size: 34px; line-height: 1.2; margin: 0 0 10px; letter-spacing: -0.02em; }
        .__fr_meta { font-family: -apple-system, "Segoe UI", sans-serif; font-size: 12.5px; opacity: 0.55; margin-bottom: 32px; }
        .__fr_article h2, .__fr_article h3 { margin: 28px 0 12px; line-height: 1.3; }
        .__fr_article p { margin: 0 0 18px; }
        .__fr_article blockquote { margin: 20px 0; padding: 6px 18px; border-left: 3px solid rgba(128,128,128,0.45); opacity: 0.85; font-style: italic; }
        .__fr_article pre { background: rgba(128,128,128,0.12); padding: 14px; border-radius: 10px; overflow-x: auto; font-family: Consolas, monospace; font-size: 14px; }
      </style>
      <div class="__fr_bar">
        <span style="opacity:0.65;font-weight:500;">Modo Lectura</span>
        <div class="__fr_controls">
          <button class="__fr_btn" id="__fr_sm">A-</button>
          <button class="__fr_btn" id="__fr_lg">A+</button>
          <button class="__fr_btn" data-th="obsidian">Negro</button>
          <button class="__fr_btn" data-th="sepia">Sepia</button>
          <button class="__fr_btn" data-th="light">Claro</button>
          <button class="__fr_btn" id="__fr_close">Cerrar</button>
        </div>
      </div>
      <article class="__fr_article" id="__fr_body">
        <h1></h1>
        <div class="__fr_meta"></div>
      </article>
    `;

    const art = overlay.querySelector('#__fr_body');
    art.querySelector('h1').textContent = title;
    art.querySelector('.__fr_meta').textContent = `${location.hostname} · ${readMins} min de lectura`;
    if (!blocks.length) {
      const p = document.createElement('p');
      p.textContent = (best.innerText || '').trim().slice(0, 12000) || 'No se detectó texto legible.';
      art.appendChild(p);
    } else {
      for (const b of blocks) {
        const node = document.createElement(b.tag === 'h1' ? 'h2' : b.tag);
        node.textContent = b.text;
        art.appendChild(node);
      }
    }

    let fontSize = 19;
    overlay.querySelector('#__fr_sm').onclick = () => { fontSize = Math.max(14, fontSize - 2); art.style.fontSize = fontSize + 'px'; };
    overlay.querySelector('#__fr_lg').onclick = () => { fontSize = Math.min(28, fontSize + 2); art.style.fontSize = fontSize + 'px'; };
    overlay.querySelectorAll('[data-th]').forEach(btn => btn.onclick = () => { overlay.dataset.theme = btn.dataset.th; });
    const close = () => { overlay.remove(); document.documentElement.style.overflow = ''; };
    overlay.querySelector('#__fr_close').onclick = close;
    window.addEventListener('keydown', function onEsc(e) {
      if (e.key === 'Escape' && document.getElementById('__ferrum_reader_overlay')) {
        close(); window.removeEventListener('keydown', onEsc);
      }
    });

    document.documentElement.style.overflow = 'hidden';
    document.body.appendChild(overlay);
    return 'true';
  };

  // --- 6. 1-CLICK PICTURE-IN-PICTURE ---
  window.__ferrumTogglePiP = function() {
    try {
      if (document.pictureInPictureElement) {
        document.exitPictureInPicture();
        return 'exited';
      }
      const videos = Array.from(document.querySelectorAll('video'))
        .filter(v => v.readyState >= 1 && v.offsetWidth > 60 && v.offsetHeight > 60)
        .sort((a, b) => (b.paused ? 0 : 1e9) - (a.paused ? 0 : 1e9) + (b.offsetWidth * b.offsetHeight - a.offsetWidth * a.offsetHeight));
      if (!videos.length) return 'no_video';
      const v = videos[0];
      v.disablePictureInPicture = false;
      v.requestPictureInPicture();
      return 'entered';
    } catch (_) {
      return 'error';
    }
  };

  // --- 7. ON-DEMAND VAULT CREDENTIAL AUTOFILL ---
  window.__ferrumAutofill = function(username, password) {
    const setVal = (el, val) => {
      if (!el) return;
      const proto = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value');
      if (proto && proto.set) proto.set.call(el, val);
      else el.value = val;
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
    };

    const passInputs = Array.from(document.querySelectorAll('input[type="password"]'))
      .filter(el => el.offsetParent !== null);
    let filled = 0;

    if (passInputs.length) {
      const pw = passInputs[0];
      setVal(pw, password);
      filled++;
      const form = pw.closest('form') || document;
      const userCandidates = Array.from(form.querySelectorAll('input[type="email"], input[type="text"], input[name*="user" i], input[name*="mail" i], input[autocomplete="username"]'))
        .filter(el => el.offsetParent !== null && el !== pw);
      if (userCandidates.length && username) {
        setVal(userCandidates[0], username);
        filled++;
      }
    } else if (username) {
      const email = document.querySelector('input[type="email"], input[autocomplete="username"], input[name*="mail" i]');
      if (email && email.offsetParent !== null) {
        setVal(email, username);
        filled++;
      }
    }
    return String(filled);
  };

  // --- 8. VISUAL ELEMENT PICKER (uBlock Origin style) ---
  window.__ferrumStartElementPicker = function() {
    if (window.__ferrumPickerActive) return;
    window.__ferrumPickerActive = true;

    const hl = document.createElement('div');
    hl.id = '__ferrum_picker_hl';
    Object.assign(hl.style, {
      position: 'fixed', pointerEvents: 'none', zIndex: '2147483646',
      border: '2px solid #ff453a', background: 'rgba(255, 69, 58, 0.16)',
      borderRadius: '4px', transition: 'all 40ms ease-out', display: 'none'
    });

    const tip = document.createElement('div');
    tip.id = '__ferrum_picker_tip';
    Object.assign(tip.style, {
      position: 'fixed', bottom: '16px', left: '50%', transform: 'translateX(-50%)',
      zIndex: '2147483647', background: '#0c0c10', color: '#fff',
      border: '1px solid rgba(255,255,255,0.2)', borderRadius: '999px',
      padding: '8px 16px', fontFamily: '-apple-system, Segoe UI, sans-serif',
      fontSize: '12px', boxShadow: '0 8px 28px rgba(0,0,0,0.75)',
      pointerEvents: 'none', maxWidth: '80vw', whiteSpace: 'nowrap',
      overflow: 'hidden', textOverflow: 'ellipsis'
    });
    tip.textContent = 'Selector de Escudo · Haz clic en un elemento para ocultarlo · Esc para cancelar';

    document.documentElement.appendChild(hl);
    document.documentElement.appendChild(tip);

    let currentTarget = null;

    function buildSelector(el) {
      if (!el || el === document.body || el === document.documentElement) return '';
      if (el.id && /^[a-zA-Z][\w-]{1,50}$/.test(el.id)) {
        return '#' + CSS.escape(el.id);
      }
      const tag = el.tagName.toLowerCase();
      const classes = Array.from(el.classList || [])
        .filter(c => c.length >= 2 && c.length <= 40 && !/^(active|open|show|visible|focus|hover|ng-|css-)/.test(c))
        .slice(0, 2);
      if (classes.length) {
        const candidate = tag + '.' + classes.map(c => CSS.escape(c)).join('.');
        try {
          if (document.querySelectorAll(candidate).length <= 12) return candidate;
        } catch (_) {}
      }
      const parts = [];
      let node = el;
      for (let d = 0; d < 4 && node && node.nodeType === 1 && node !== document.body; d++) {
        let seg = node.tagName.toLowerCase();
        if (node.id && /^[a-zA-Z][\w-]{1,40}$/.test(node.id)) {
          parts.unshift('#' + CSS.escape(node.id));
          break;
        }
        const p = node.parentElement;
        if (p) {
          const same = Array.from(p.children).filter(c => c.tagName === node.tagName);
          if (same.length > 1) {
            seg += ':nth-of-type(' + (same.indexOf(node) + 1) + ')';
          }
        }
        parts.unshift(seg);
        node = p;
      }
      return parts.join(' > ');
    }

    function cleanup() {
      window.__ferrumPickerActive = false;
      hl.remove();
      tip.remove();
      document.removeEventListener('mousemove', onMove, true);
      document.removeEventListener('click', onClick, true);
      document.removeEventListener('keydown', onKey, true);
    }

    function onMove(e) {
      const el = document.elementFromPoint(e.clientX, e.clientY);
      if (!el || el === hl || el === tip || el === document.body || el === document.documentElement) return;
      currentTarget = el;
      const r = el.getBoundingClientRect();
      hl.style.display = 'block';
      hl.style.left = r.left + 'px';
      hl.style.top = r.top + 'px';
      hl.style.width = Math.max(4, r.width) + 'px';
      hl.style.height = Math.max(4, r.height) + 'px';
      const sel = buildSelector(el);
      tip.textContent = 'Bloquear: ' + (sel || el.tagName.toLowerCase()) + ' · Esc para cancelar';
    }

    function onClick(e) {
      e.preventDefault();
      e.stopPropagation();
      e.stopImmediatePropagation();
      const target = currentTarget || e.target;
      const sel = buildSelector(target);
      cleanup();
      if (sel) {
        try { target.style.setProperty('display', 'none', 'important'); } catch (_) {}
        postBridge({ kind: 'element_picked', host: location.hostname, selector: sel });
      }
    }

    function onKey(e) {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        cleanup();
      }
    }

    document.addEventListener('mousemove', onMove, true);
    document.addEventListener('click', onClick, true);
    document.addEventListener('keydown', onKey, true);
  };

  // --- 9. AUTOMATIC LOGIN FORM DETECTION (Non-intrusive Vault Save Offer) ---
  let lastReportedLogin = '';
  function captureLoginFromRoot(root) {
    try {
      if (!location.hostname || location.protocol === 'file:') return;
      const scope = root && root.querySelectorAll ? root : document;
      const pw = Array.from(scope.querySelectorAll('input[type="password"]'))
        .find(el => el.value && el.value.length >= 3);
      if (!pw) return;
      const form = pw.closest('form') || document;
      const userEl = Array.from(form.querySelectorAll('input[type="email"], input[type="text"], input[name*="user" i], input[name*="mail" i], input[autocomplete="username"]'))
        .find(el => el !== pw && el.value && el.value.trim().length > 0);
      const username = userEl ? userEl.value.trim() : '';
      const password = pw.value;
      const sig = location.hostname + '|' + username + '|' + password.length;
      if (sig === lastReportedLogin) return;
      lastReportedLogin = sig;
      postBridge({
        kind: 'login_detected',
        host: location.hostname,
        username,
        password
      });
    } catch (_) {}
  }

  document.addEventListener('submit', (e) => captureLoginFromRoot(e.target), true);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && e.target && e.target.type === 'password' && e.target.value) {
      captureLoginFromRoot(e.target.closest('form') || document);
    }
  }, true);

  // --- 10. ISOLATED HOVER LINK PREVIEW & BOTTOM-RIGHT TOAST OVERLAY (Top Frame Only) ---
  if (window.top === window) {
    let uiShadow = null;
    let linkBar = null;
    let linkText = null;
    let toastBox = null;
    let toastHideTimer = null;
    let linkShowTimer = null;

    function ensureOverlayHost() {
      if (uiShadow && document.documentElement.contains(uiShadow.__host)) return true;
      if (!document.documentElement) return false;
      const host = document.createElement('div');
      host.setAttribute('data-ferrum-overlay', '1');
      host.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483647;margin:0;padding:0;border:0;background:transparent;';
      const shadow = host.attachShadow({ mode: 'closed' });
      shadow.__host = host;
      const st = document.createElement('style');
      st.textContent = `
        :host { all: initial; }
        .f-link {
          position: fixed;
          left: 8px;
          bottom: 8px;
          max-width: min(520px, 68vw);
          padding: 4px 10px;
          border-radius: 7px;
          background: rgba(11, 11, 15, 0.92);
          border: 1px solid rgba(255, 255, 255, 0.12);
          box-shadow: 0 6px 20px rgba(0, 0, 0, 0.55);
          color: rgba(255, 255, 255, 0.86);
          font: 450 11.5px/1.3 -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI Variable", system-ui, sans-serif;
          white-space: nowrap;
          overflow: hidden;
          text-overflow: ellipsis;
          opacity: 0;
          transform: translateY(3px);
          transition: opacity 110ms ease, transform 110ms ease;
          pointer-events: none;
          -webkit-font-smoothing: antialiased;
        }
        .f-link.show { opacity: 1; transform: translateY(0); }
        .f-link.insecure { border-color: rgba(255, 159, 10, 0.45); color: #ffd180; }
        .f-toast {
          position: fixed;
          right: 16px;
          bottom: 16px;
          max-width: min(360px, 76vw);
          padding: 8px 14px;
          border-radius: 10px;
          background: rgba(20, 20, 26, 0.95);
          border: 1px solid rgba(255, 255, 255, 0.16);
          box-shadow: 0 10px 30px rgba(0, 0, 0, 0.65), inset 0 1px 0 rgba(255, 255, 255, 0.08);
          color: #ffffff;
          font: 500 12px/1.35 -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI Variable", system-ui, sans-serif;
          letter-spacing: -0.01em;
          opacity: 0;
          transform: translateY(8px);
          transition: opacity 160ms ease, transform 160ms ease;
          pointer-events: none;
          -webkit-font-smoothing: antialiased;
        }
        .f-toast.show { opacity: 1; transform: translateY(0); }
      `;
      linkBar = document.createElement('div');
      linkBar.className = 'f-link';
      linkText = document.createElement('span');
      linkBar.appendChild(linkText);
      toastBox = document.createElement('div');
      toastBox.className = 'f-toast';
      shadow.append(st, linkBar, toastBox);
      document.documentElement.appendChild(host);
      uiShadow = shadow;
      return true;
    }

    function hideLinkPreview() {
      clearTimeout(linkShowTimer);
      if (linkBar) linkBar.classList.remove('show');
    }

    function showLinkPreviewFor(el) {
      const a = el && el.closest ? el.closest('a[href]') : null;
      if (!a) { hideLinkPreview(); return; }
      const rawHref = a.getAttribute('href') || '';
      if (!rawHref || rawHref.startsWith('#') || rawHref.toLowerCase().startsWith('javascript:')) {
        hideLinkPreview();
        return;
      }
      let href = a.href || rawHref;
      if (href.length > 160) href = href.slice(0, 159) + '…';
      clearTimeout(linkShowTimer);
      linkShowTimer = setTimeout(() => {
        if (!ensureOverlayHost()) return;
        linkText.textContent = href;
        linkBar.classList.toggle('insecure', href.startsWith('http://'));
        linkBar.classList.add('show');
      }, 90);
    }

    document.addEventListener('mouseover', (e) => showLinkPreviewFor(e.target), { passive: true, capture: true });
    document.addEventListener('mouseout', (e) => {
      const a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
      if (a && (!e.relatedTarget || !a.contains(e.relatedTarget))) hideLinkPreview();
    }, { passive: true, capture: true });
    document.addEventListener('focusin', (e) => showLinkPreviewFor(e.target), { passive: true, capture: true });
    document.addEventListener('focusout', hideLinkPreview, { passive: true, capture: true });
    document.addEventListener('mousedown', hideLinkPreview, { passive: true, capture: true });
    window.addEventListener('scroll', hideLinkPreview, { passive: true });
    window.addEventListener('blur', hideLinkPreview, { passive: true });

    window.__ferrumShowToast = function(msg) {
      if (!msg || !ensureOverlayHost()) return;
      toastBox.textContent = String(msg);
      toastBox.classList.add('show');
      clearTimeout(toastHideTimer);
      toastHideTimer = setTimeout(() => toastBox.classList.remove('show'), 2600);
    };
  }

  const boot = () => {
    if (!window.__ferrumCosmeticDisabled) applyStyle('__ferrum_cosmetic_style', COSMETIC_CSS, true);
    if (!window.__ferrumCookieBlockDisabled) applyStyle('__ferrum_cookie_style', COOKIE_BANNER_CSS, true);
  };
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot, { once: true });
  } else {
    boot();
  }
})();
"#;

const URL_SUBSTRING_RULES: &[(&str, BlockCategory, bool)] = &[
    ("/pagead/", BlockCategory::Ad, false),
    ("/adservice/", BlockCategory::Ad, false),
    ("/ads/ad", BlockCategory::Ad, true),
    ("/adserver/", BlockCategory::Ad, true),
    ("/adclick", BlockCategory::Ad, true),
    ("/doubleclick/", BlockCategory::Ad, false),
    ("/prebid", BlockCategory::Ad, true),
    ("/gpt/pubads", BlockCategory::Ad, false),
    ("show_ads.js", BlockCategory::Ad, false),
    ("adsbygoogle.js", BlockCategory::Ad, false),
    ("/api/stats/ads", BlockCategory::Ad, false),
    ("/pagead/interaction", BlockCategory::Ad, false),
    ("/g/collect?", BlockCategory::Analytics, false),
    ("/j/collect?", BlockCategory::Analytics, true),
    ("/r/collect?", BlockCategory::Analytics, true),
    ("/analytics.js", BlockCategory::Analytics, true),
    ("/gtag/js", BlockCategory::Analytics, true),
    ("/gtm.js", BlockCategory::Tracker, true),
    ("/fbevents.js", BlockCategory::Social, true),
    ("/tr/?id=", BlockCategory::Social, true),
    ("/pixel.gif", BlockCategory::Tracker, true),
    ("/pixel.png", BlockCategory::Tracker, true),
    ("/telemetry/", BlockCategory::Tracker, true),
    ("/beacon?", BlockCategory::Tracker, true),
    ("/track/event", BlockCategory::Tracker, true),
    ("/sponsored_content", BlockCategory::Ad, true),
    ("coinhive.min.js", BlockCategory::Security, false),
    ("cryptonight.wasm", BlockCategory::Security, false),
];

const SECURITY_MALWARE_DOMAINS: &[&str] = &[
    "coinhive.com",
    "coin-hive.com",
    "authedmine.com",
    "cryptoloot.pro",
    "crypto-loot.com",
    "jsecoin.com",
    "minero.cc",
    "webminepool.com",
    "ppoi.org",
    "monerominer.rocks",
    "papoto.com",
    "afminer.com",
    "coinimp.com",
    "browsermine.com",
    "miner.pr0gramm.com",
    "deepminer.org",
    "hashing.win",
    "webmine.cz",
    "minemytraffic.com",
    "2giga.link",
    "ad-maven.com",
    "admaven.com",
    "clickadu.com",
    "hilltopads.net",
    "pushame.com",
    "pushwhy.com",
    "onclickalgo.com",
    "onclkds.com",
    "deloplen.com",
    "dolohen.com",
];

const AD_DOMAINS: &[&str] = &[
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "adservice.google.com",
    "pagead2.googlesyndication.com",
    "tpc.googlesyndication.com",
    "fundingchoicesmessages.google.com",
    "ads.google.com",
    "adclick.g.doubleclick.net",
    "pubads.g.doubleclick.net",
    "securepubads.g.doubleclick.net",
    "cm.g.doubleclick.net",
    "stats.g.doubleclick.net",
    "amazon-adsystem.com",
    "aax.amazon-adsystem.com",
    "c.amazon-adsystem.com",
    "fls-na.amazon-adsystem.com",
    "adnxs.com",
    "ib.adnxs.com",
    "acdn.adnxs.com",
    "criteo.com",
    "criteo.net",
    "static.criteo.net",
    "bidder.criteo.com",
    "taboola.com",
    "cdn.taboola.com",
    "trc.taboola.com",
    "outbrain.com",
    "widgets.outbrain.com",
    "log.outbrainimg.com",
    "pubmatic.com",
    "ads.pubmatic.com",
    "hbopenbid.pubmatic.com",
    "rubiconproject.com",
    "fastlane.rubiconproject.com",
    "pixel.rubiconproject.com",
    "openx.net",
    "rtb.openx.net",
    "us-u.openx.net",
    "casalemedia.com",
    "as-sec.casalemedia.com",
    "indexww.com",
    "js-sec.indexww.com",
    "smartadserver.com",
    "diff.smartadserver.com",
    "adform.net",
    "track.adform.net",
    "serving-sys.com",
    "bs.serving-sys.com",
    "advertising.com",
    "adtech.de",
    "adtechus.com",
    "adsrvr.org",
    "match.adsrvr.org",
    "insight.adsrvr.org",
    "media.net",
    "contextual.media.net",
    "bidswitch.net",
    "x.bidswitch.net",
    "triplelift.com",
    "tlx.3lift.com",
    "3lift.com",
    "sharethrough.com",
    "btlr.sharethrough.com",
    "sovrn.com",
    "ap.lijit.com",
    "lijit.com",
    "teads.tv",
    "a.teads.tv",
    "t.teads.tv",
    "yieldmo.com",
    "ads.yieldmo.com",
    "gumgum.com",
    "g2.gumgum.com",
    "revcontent.com",
    "trends.revcontent.com",
    "mgid.com",
    "servicer.mgid.com",
    "popads.net",
    "c1.popads.net",
    "popcash.net",
    "propellerads.com",
    "propellerclick.com",
    "exoclick.com",
    "syndication.exoclick.com",
    "juicyads.com",
    "trafficjunky.net",
    "adsterra.com",
    "highcpmgate.com",
    "unityads.unity3d.com",
    "applovin.com",
    "vungle.com",
    "inmobi.com",
    "mopub.com",
    "flurry.com",
    "moatads.com",
    "z.moatads.com",
    "doubleverify.com",
    "cdn.doubleverify.com",
    "adsafeprotected.com",
    "pixel.adsafeprotected.com",
    "fwmrm.net",
    "innovid.com",
    "spotxchange.com",
    "springserve.com",
    "connatix.com",
    "kargo.com",
    "adroll.com",
    "d.adroll.com",
    "s.adroll.com",
    "zemanta.com",
    "b1sync.zemanta.com",
    "medianet.com",
    "adblade.com",
    "buysellads.com",
    "srv.buysellads.com",
    "carbonads.com",
    "srv.carbonads.net",
    "ethicalads.io",
    "ads.yahoo.com",
    "adserver.yahoo.com",
    "gemini.yahoo.com",
    "ads.linkedin.com",
    "ads-twitter.com",
    "static.ads-twitter.com",
    "ads.pinterest.com",
    "ads.tiktok.com",
    "ads.reddit.com",
    "alb.reddit.com",
    "events.redditmedia.com",
    "bat.bing.com",
    "c.bing.com",
    "ads.microsoft.com",
    "adnxs-simple.com",
    "richaudience.com",
    "seedtag.com",
    "aniview.com",
    "vidazoo.com",
    "primis.tech",
    "minute.ly",
    "anyclip.com",
    "exelator.com",
    "bluekai.com",
    "tags.bluekai.com",
    "krxd.net",
    "cdn.krxd.net",
    "demdex.net",
    "dpm.demdex.net",
    "omtrdc.net",
    "everesttech.net",
    "rlcdn.com",
    "idsync.rlcdn.com",
    "tapad.com",
    "pixel.tapad.com",
    "crwdcntrl.net",
    "bcp.crwdcntrl.net",
    "eyeota.net",
    "agkn.com",
    "liadm.com",
    "idx.liadm.com",
    "id5-sync.com",
    "intentiq.com",
    "audigent.com",
    "narrative.io",
    "permutive.com",
    "zeotap.com",
];

const TRACKER_DOMAINS: &[&str] = &[
    "googletagmanager.com",
    "scorecardresearch.com",
    "sb.scorecardresearch.com",
    "quantserve.com",
    "pixel.quantserve.com",
    "quantcount.com",
    "hotjar.com",
    "static.hotjar.com",
    "script.hotjar.com",
    "vars.hotjar.com",
    "fullstory.com",
    "edge.fullstory.com",
    "rs.fullstory.com",
    "mouseflow.com",
    "crazyegg.com",
    "script.crazyegg.com",
    "luckyorange.com",
    "clarity.ms",
    "www.clarity.ms",
    "c.clarity.ms",
    "logrocket.io",
    "cdn.logrocket.io",
    "smartlook.com",
    "rec.smartlook.com",
    "contentsquare.net",
    "t.contentsquare.net",
    "quantummetric.com",
    "browser-intake-datadoghq.com",
    "rum.browser-intake-datadoghq.com",
    "js-agent.newrelic.com",
    "bam.nr-data.net",
    "bam-cell.nr-data.net",
    "bugsnag.com",
    "notify.bugsnag.com",
    "rollbar.com",
    "branch.io",
    "api2.branch.io",
    "app.link",
    "appsflyer.com",
    "adjust.com",
    "kochava.com",
    "singular.net",
    "braze.com",
    "sdk.iad-01.braze.com",
    "onesignal.com",
    "cdn.onesignal.com",
    "clevertap.com",
    "moengage.com",
    "webengage.com",
    "hubspot.com",
    "js.hs-analytics.net",
    "js.hs-scripts.com",
    "js.hs-banner.com",
    "track.hubspot.com",
    "mktoresp.com",
    "munchkin.marketo.net",
    "pardot.com",
    "pi.pardot.com",
    "bizible.com",
    "cdn.bizible.com",
    "6sc.co",
    "j.6sc.co",
    "clearbit.com",
    "x.clearbitjs.com",
    "drift.com",
    "intercom.io",
    "api-iam.intercom.io",
    "optimizely.com",
    "logx.optimizely.com",
    "vwo.com",
    "dev.visualwebsiteoptimizer.com",
    "abtasty.com",
    "kameleoon.eu",
    "cookielaw.org",
    "cdn.cookielaw.org",
    "onetrust.com",
    "trustarc.com",
    "consent.trustarc.com",
    "quantcast.mgr.consensu.org",
    "privacy-mgmt.com",
    "didomi.io",
    "sdk.privacy-center.org",
    "consent.cookiebot.com",
    "app.usercentrics.eu",
    "bounceexchange.com",
    "tag.bounceexchange.com",
    "wknd.ai",
    "tag.wknd.ai",
    "skimresources.com",
    "s.skimresources.com",
    "viglink.com",
    "cdn.viglink.com",
    "narrativ.com",
    "bam-x.com",
    "collector.github.com",
    "events.data.microsoft.com",
    "vortex.data.microsoft.com",
    "telemetry.mozilla.org",
];

const ANALYTICS_DOMAINS: &[&str] = &[
    "google-analytics.com",
    "ssl.google-analytics.com",
    "www.google-analytics.com",
    "region1.google-analytics.com",
    "analytics.google.com",
    "stats.wp.com",
    "pixel.wp.com",
    "mixpanel.com",
    "api.mixpanel.com",
    "api-js.mixpanel.com",
    "cdn.mxpnl.com",
    "segment.io",
    "segment.com",
    "api.segment.io",
    "cdn.segment.com",
    "amplitude.com",
    "api.amplitude.com",
    "api2.amplitude.com",
    "cdn.amplitude.com",
    "heapanalytics.com",
    "cdn.heapanalytics.com",
    "pendo.io",
    "cdn.pendo.io",
    "data.pendo.io",
    "posthog.com",
    "us.i.posthog.com",
    "eu.i.posthog.com",
    "plausible.io",
    "matomo.cloud",
    "chartbeat.com",
    "chartbeat.net",
    "static.chartbeat.com",
    "ping.chartbeat.net",
    "parsely.com",
    "cdn.parsely.com",
    "p1.parsely.com",
    "kissmetrics.com",
    "i.kissmetrics.io",
    "woopra.com",
    "static.woopra.com",
    "clicky.com",
    "static.getclicky.com",
    "statcounter.com",
    "c.statcounter.com",
    "histats.com",
    "s10.histats.com",
    "mc.yandex.ru",
    "mc.yandex.com",
    "hm.baidu.com",
    "cnzz.com",
    "umeng.com",
    "tiqcdn.com",
    "tags.tiqcdn.com",
    "ensighten.com",
    "nexus.ensighten.com",
    "adobedtm.com",
    "assets.adobedtm.com",
    "2o7.net",
    "sc.omtrdc.net",
    "snowplowanalytics.com",
    "rudderstack.com",
    "rudderlabs.com",
    "collector.app.rudderstack.com",
    "mparticle.com",
    "jssdks.mparticle.com",
    "tealiumiq.com",
    "collect.tealiumiq.com",
];

const SOCIAL_TRACKER_DOMAINS: &[&str] = &[
    "connect.facebook.net",
    "pixel.facebook.com",
    "an.facebook.com",
    "graph.facebook.com",
    "analytics.tiktok.com",
    "business-api.tiktok.com",
    "snap.licdn.com",
    "px.ads.linkedin.com",
    "ct.pinterest.com",
    "tr.snapchat.com",
    "sc-static.net",
    "analytics.twitter.com",
    "t.co",
    "platform.twitter.com",
    "syndication.twitter.com",
    "pixel.reddit.com",
    "addthis.com",
    "s7.addthis.com",
    "sharethis.com",
    "platform-api.sharethis.com",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_ads_trackers_cryptominers_and_adblock_rules() {
        let dir = std::env::temp_dir().join("ferrum_ab_empty_test");
        let mut engine = AdBlockEngine::new(&BrowserSettings::default(), &dir);

        assert_eq!(
            engine
                .check_request(
                    "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
                    Some("example.com"),
                    "script",
                    false
                )
                .map(|d| d.category),
            Some(BlockCategory::Ad)
        );
        assert_eq!(
            engine
                .check_request(
                    "https://coinhive.com/lib/coinhive.min.js",
                    Some("example.com"),
                    "script",
                    false
                )
                .map(|d| d.category),
            Some(BlockCategory::Security)
        );

        // Verify Brave adblock-rust engine integration + serialization round-trip
        let mut fs_set = FilterSet::new(false);
        fs_set.add_filter_list(
            "||custom-ad-network.test^$third-party".to_string(),
            ParseOptions::default(),
        );
        let brave = BraveAdblockEngine::new_with_filter_set(fs_set);
        let bytes = brave.serialize();
        let mut restored = BraveAdblockEngine::default();
        restored.deserialize(&bytes).unwrap();
        engine.install_brave_engine(restored, 1);

        assert_eq!(
            engine
                .check_request(
                    "https://cdn.custom-ad-network.test/banner.js",
                    Some("news.org"),
                    "script",
                    false
                )
                .map(|d| d.category),
            Some(BlockCategory::EasyList)
        );
    }
}
