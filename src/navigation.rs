use serde::{Deserialize, Serialize};
use std::path::Path;
use url::Url;

use crate::storage::CustomSearchEngine;

pub const INTERNAL_PROTOCOL: &str = "ferrum";
pub const INTERNAL_NEWTAB_DISPLAY: &str = "ferrum://newtab";
pub const INTERNAL_NEWTAB_URL: &str = "ferrum://localhost/newtab";
pub const INTERNAL_ORIGIN_PREFIX: &str = "http://ferrum.localhost/";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchEngine {
    #[default]
    DuckDuckGo,
    Google,
    Brave,
    Startpage,
    Bing,
}

impl SearchEngine {
    pub fn from_id(id: &str) -> Self {
        match id.to_lowercase().as_str() {
            "google" => Self::Google,
            "brave" => Self::Brave,
            "startpage" => Self::Startpage,
            "bing" => Self::Bing,
            _ => Self::DuckDuckGo,
        }
    }

    pub fn search_url(&self, query: &str) -> String {
        let encoded = percent_encode_query(query);
        match self {
            Self::DuckDuckGo => format!("https://duckduckgo.com/?q={encoded}"),
            Self::Google => format!("https://www.google.com/search?q={encoded}"),
            Self::Brave => format!("https://search.brave.com/search?q={encoded}"),
            Self::Startpage => format!("https://www.startpage.com/sp/search?query={encoded}"),
            Self::Bing => format!("https://www.bing.com/search?q={encoded}"),
        }
    }
}

pub fn percent_encode_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            b => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

pub fn to_display_url(raw: &str) -> String {
    if raw.is_empty() || raw == "about:blank" {
        return INTERNAL_NEWTAB_DISPLAY.to_string();
    }
    if let Some(rest) = raw.strip_prefix(INTERNAL_ORIGIN_PREFIX) {
        return format!("{INTERNAL_PROTOCOL}://{}", rest.trim_start_matches('/'));
    }
    if let Some(rest) = raw.strip_prefix("https://ferrum.localhost/") {
        return format!("{INTERNAL_PROTOCOL}://{}", rest.trim_start_matches('/'));
    }
    if let Some(rest) = raw.strip_prefix("ferrum://localhost/") {
        return format!("{INTERNAL_PROTOCOL}://{}", rest.trim_start_matches('/'));
    }
    raw.to_string()
}

pub fn is_internal_url(url: &str) -> bool {
    url.starts_with("ferrum://")
        || url.starts_with(INTERNAL_ORIGIN_PREFIX)
        || url.starts_with("https://ferrum.localhost/")
        || url == "about:blank"
}

pub fn extract_host(raw: &str) -> Option<String> {
    if is_internal_url(raw) {
        return None;
    }
    Url::parse(raw)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
}

pub fn is_local_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h == "localhost"
        || h == "127.0.0.1"
        || h == "0.0.0.0"
        || h == "::1"
        || h.starts_with("192.168.")
        || h.starts_with("10.")
        || h.ends_with(".local")
        || h.ends_with(".localhost")
}

pub fn upgrade_to_https_if_enabled(raw_url: &str, https_only: bool) -> String {
    if !https_only || !raw_url.starts_with("http://") || is_internal_url(raw_url) {
        return raw_url.to_string();
    }
    let Ok(mut parsed) = Url::parse(raw_url) else {
        return raw_url.to_string();
    };
    if let Some(host) = parsed.host_str()
        && !is_local_host(host)
    {
        let _ = parsed.set_scheme("https");
        return parsed.to_string();
    }
    raw_url.to_string()
}

pub fn strip_tracking_params(raw_url: &str) -> String {
    const TRACKING_PARAMS: &[&str] = &[
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "utm_term",
        "utm_content",
        "utm_id",
        "utm_cid",
        "utm_reader",
        "utm_name",
        "fbclid",
        "gclid",
        "gbraid",
        "wbraid",
        "dclid",
        "msclkid",
        "twclid",
        "igshid",
        "mc_cid",
        "mc_eid",
        "yclid",
        "_hsenc",
        "_hsmi",
        "mkt_tok",
        "vero_id",
        "ref_src",
        "ref_url",
        "s_cid",
        "spm",
        "oly_anon_id",
        "oly_enc_id",
        "__twitter_impression",
    ];

    let Ok(mut parsed) = Url::parse(raw_url) else {
        return raw_url.to_string();
    };
    if parsed.query().is_none() {
        return raw_url.to_string();
    }

    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| {
            let lower = k.to_ascii_lowercase();
            !TRACKING_PARAMS.contains(&lower.as_str())
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        let mut pairs = parsed.query_pairs_mut();
        pairs.clear();
        for (k, v) in kept {
            pairs.append_pair(&k, &v);
        }
    }

    parsed.to_string()
}

pub fn sanitize_navigation_url(raw_url: &str, https_only: bool, strip_trackers: bool) -> String {
    let upgraded = upgrade_to_https_if_enabled(raw_url, https_only);
    if strip_trackers {
        strip_tracking_params(&upgraded)
    } else {
        upgraded
    }
}

/// Resolve a CLI argument (URL or local `.html`/`.pdf`/image file path) to a navigable URL.
pub fn resolve_cli_target(arg: &str, https_only: bool, strip_trackers: bool) -> Option<String> {
    let trimmed = arg.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("file://")
    {
        return Some(sanitize_navigation_url(trimmed, https_only, strip_trackers));
    }
    let p = Path::new(trimmed);
    if p.exists()
        && let Ok(abs) = std::fs::canonicalize(p)
        && let Ok(file_url) = Url::from_file_path(abs)
    {
        return Some(file_url.to_string());
    }
    if !trimmed.chars().any(char::is_whitespace) && looks_like_url(trimmed) {
        return Some(resolve_omnibox_input(
            trimmed,
            SearchEngine::DuckDuckGo,
            https_only,
            strip_trackers,
        ));
    }
    None
}

/// Matches built-in or user-defined search bangs (e.g. `!yt lo-fi`, `gh wry`, `!py requests`).
/// Returns `(engine_name, raw_query, resolved_url)`.
pub fn match_search_bang(
    input: &str,
    custom_engines: &[CustomSearchEngine],
) -> Option<(String, String, String)> {
    let trimmed = input.trim();
    let (prefix, rest) = trimmed.split_once(' ')?;
    let query = rest.trim();
    if query.is_empty() {
        return None;
    }
    let encoded = percent_encode_query(query);
    let p_low = prefix.to_ascii_lowercase();
    let kw = p_low.trim_start_matches('!');

    // Check user-defined custom search shortcuts first so users can override defaults
    if let Some(custom) = custom_engines
        .iter()
        .find(|e| e.keyword.eq_ignore_ascii_case(kw))
    {
        let resolved = custom.url_template.replace("%s", &encoded);
        return Some((custom.name.clone(), query.to_string(), resolved));
    }

    let built_in: Option<(&str, String)> = match p_low.as_str() {
        "g" | "!g" => Some((
            "Google",
            format!("https://www.google.com/search?q={encoded}"),
        )),
        "ddg" | "!ddg" => Some(("DuckDuckGo", format!("https://duckduckgo.com/?q={encoded}"))),
        "brave" | "!b" | "!brave" => Some((
            "Brave Search",
            format!("https://search.brave.com/search?q={encoded}"),
        )),
        "gh" | "!gh" => Some(("GitHub", format!("https://github.com/search?q={encoded}"))),
        "yt" | "!yt" => Some((
            "YouTube",
            format!("https://www.youtube.com/results?search_query={encoded}"),
        )),
        "w" | "!w" => Some((
            "Wikipedia",
            format!("https://es.wikipedia.org/w/index.php?search={encoded}"),
        )),
        "crates" | "!c" | "!rs" => {
            Some(("crates.io", format!("https://crates.io/search?q={encoded}")))
        }
        "mdn" | "!mdn" => Some((
            "MDN Web Docs",
            format!("https://developer.mozilla.org/en-US/search?q={encoded}"),
        )),
        "so" | "!so" => Some((
            "Stack Overflow",
            format!("https://stackoverflow.com/search?q={encoded}"),
        )),
        "npm" | "!npm" => Some(("npm", format!("https://www.npmjs.com/search?q={encoded}"))),
        "r" | "!r" | "!reddit" => Some((
            "Reddit",
            format!("https://www.reddit.com/search/?q={encoded}"),
        )),
        "tr" | "!tr" => Some((
            "Traductor",
            format!("https://translate.google.com/?sl=auto&tl=es&text={encoded}"),
        )),
        "maps" | "!maps" => Some((
            "Google Maps",
            format!("https://www.google.com/maps/search/{encoded}"),
        )),
        "amz" | "!amz" => Some(("Amazon", format!("https://www.amazon.com/s?k={encoded}"))),
        _ => None,
    };

    built_in.map(|(name, url)| (name.to_string(), query.to_string(), url))
}

pub fn resolve_omnibox_input(
    input: &str,
    engine: SearchEngine,
    https_only: bool,
    strip_trackers: bool,
) -> String {
    resolve_omnibox_input_with_custom(input, engine, &[], https_only, strip_trackers)
}

pub fn resolve_omnibox_input_with_custom(
    input: &str,
    engine: SearchEngine,
    custom_engines: &[CustomSearchEngine],
    https_only: bool,
    strip_trackers: bool,
) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return INTERNAL_NEWTAB_URL.to_string();
    }

    if trimmed.eq_ignore_ascii_case("ferrum://newtab")
        || trimmed.eq_ignore_ascii_case("ferrum://home")
        || trimmed.eq_ignore_ascii_case("about:newtab")
    {
        return INTERNAL_NEWTAB_URL.to_string();
    }

    // Local existing file path typed directly into the omnibox
    let as_path = Path::new(trimmed);
    if as_path.is_absolute()
        && as_path.exists()
        && let Ok(file_url) = Url::from_file_path(as_path)
    {
        return file_url.to_string();
    }

    // Quick search bangs (built-in + custom engines)
    if let Some((_, _, bang_url)) = match_search_bang(trimmed, custom_engines) {
        return bang_url;
    }

    if trimmed.contains("://")
        && let Ok(parsed) = Url::parse(trimmed)
    {
        return sanitize_navigation_url(parsed.as_str(), https_only, strip_trackers);
    }

    if !trimmed.chars().any(char::is_whitespace) && looks_like_url(trimmed) {
        let scheme = if is_local_address(trimmed) {
            "http"
        } else {
            "https"
        };
        let candidate = format!("{scheme}://{trimmed}");
        if let Ok(parsed) = Url::parse(&candidate) {
            return sanitize_navigation_url(parsed.as_str(), https_only, strip_trackers);
        }
    }

    engine.search_url(trimmed)
}

fn is_local_address(input: &str) -> bool {
    let host_part = input.split('/').next().unwrap_or(input);
    let host = host_part.split(':').next().unwrap_or(host_part);
    is_local_host(host)
}

fn looks_like_url(input: &str) -> bool {
    let host_part = input.split('/').next().unwrap_or(input);
    if host_part.is_empty() {
        return false;
    }

    let (host, port_opt) = match host_part.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (host_part, None),
    };

    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    let octets: Vec<&str> = host.split('.').collect();
    if octets.len() == 4 && octets.iter().all(|o| o.parse::<u8>().is_ok()) {
        return true;
    }

    if let Some((name, tld)) = host.rsplit_once('.') {
        let valid_name = !name.is_empty()
            && !name.starts_with('.')
            && !name.ends_with('.')
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
        let valid_tld =
            tld.len() >= 2 && tld.len() <= 24 && tld.chars().all(|c| c.is_ascii_alphabetic());
        if valid_name && valid_tld {
            return true;
        }
    }

    port_opt.is_some() && !host.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_domains_urls_and_https_upgrade() {
        assert_eq!(
            resolve_omnibox_input("rust-lang.org", SearchEngine::DuckDuckGo, true, true),
            "https://rust-lang.org/"
        );
        assert_eq!(
            resolve_omnibox_input("localhost:3000/api", SearchEngine::DuckDuckGo, true, true),
            "http://localhost:3000/api"
        );
        assert_eq!(
            resolve_omnibox_input(
                "http://example.com/?a=1&utm_source=twitter&fbclid=abc",
                SearchEngine::Google,
                true,
                true
            ),
            "https://example.com/?a=1"
        );
    }

    #[test]
    fn resolves_search_queries_and_bangs() {
        assert_eq!(
            resolve_omnibox_input(
                "como hacer un navegador en rust",
                SearchEngine::DuckDuckGo,
                true,
                true
            ),
            "https://duckduckgo.com/?q=como+hacer+un+navegador+en+rust"
        );
        assert_eq!(
            resolve_omnibox_input("gh wry", SearchEngine::DuckDuckGo, true, true),
            "https://github.com/search?q=wry"
        );
        assert_eq!(
            resolve_omnibox_input("mdn fetch", SearchEngine::DuckDuckGo, true, true),
            "https://developer.mozilla.org/en-US/search?q=fetch"
        );
        assert_eq!(
            resolve_omnibox_input("ferrum://newtab", SearchEngine::DuckDuckGo, true, true),
            INTERNAL_NEWTAB_URL
        );
        let custom = vec![CustomSearchEngine {
            keyword: "py".into(),
            name: "PyPI".into(),
            url_template: "https://pypi.org/search/?q=%s".into(),
        }];
        assert_eq!(
            resolve_omnibox_input_with_custom(
                "!py requests",
                SearchEngine::DuckDuckGo,
                &custom,
                true,
                true
            ),
            "https://pypi.org/search/?q=requests"
        );
        assert_eq!(
            resolve_omnibox_input("!npm vite", SearchEngine::DuckDuckGo, true, true),
            "https://www.npmjs.com/search?q=vite"
        );
    }
}
