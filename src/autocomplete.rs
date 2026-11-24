use serde::Serialize;
use std::collections::HashSet;
use std::time::Duration;

use crate::navigation::{SearchEngine, is_internal_url, percent_encode_query};
use crate::storage::{Bookmark, HistoryEntry};
use crate::ui::TabSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKind {
    SwitchTab,
    Bookmark,
    History,
    Search,
    Navigate,
}

#[derive(Debug, Clone, Serialize)]
pub struct OmniboxSuggestion {
    pub kind: SuggestionKind,
    pub title: String,
    pub subtitle: String,
    pub value: String,
    pub tab_id: Option<u64>,
}

pub fn build_suggestions(
    raw_query: &str,
    tabs: &[TabSummary],
    active_tab_id: u64,
    bookmarks: &[Bookmark],
    history: &[HistoryEntry],
    engine: SearchEngine,
    web_suggestions: &[String],
) -> Vec<OmniboxSuggestion> {
    let q = raw_query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let ql = q.to_ascii_lowercase();
    let mut results = Vec::with_capacity(8);
    let mut seen_urls = HashSet::new();

    // 1. Direct URL or search action as top fallback/first row
    let looks_domain = !q.contains(' ') && (q.contains('.') || q.starts_with("localhost"));
    if looks_domain {
        results.push(OmniboxSuggestion {
            kind: SuggestionKind::Navigate,
            title: q.to_string(),
            subtitle: "Ir directamente al sitio".to_string(),
            value: q.to_string(),
            tab_id: None,
        });
    } else {
        let engine_label = match engine {
            SearchEngine::DuckDuckGo => "DuckDuckGo",
            SearchEngine::Google => "Google",
            SearchEngine::Brave => "Brave Search",
            SearchEngine::Startpage => "Startpage",
            SearchEngine::Bing => "Bing",
        };
        results.push(OmniboxSuggestion {
            kind: SuggestionKind::Search,
            title: q.to_string(),
            subtitle: format!("Buscar en {engine_label}"),
            value: q.to_string(),
            tab_id: None,
        });
    }

    // 2. Open tabs ("Cambiar a esta pestaña" — avoids opening duplicates!)
    for tab in tabs {
        if tab.id == active_tab_id || tab.is_internal {
            continue;
        }
        let t_low = tab.title.to_ascii_lowercase();
        let u_low = tab.display_url.to_ascii_lowercase();
        if t_low.contains(&ql) || u_low.contains(&ql) {
            seen_urls.insert(norm_key(&tab.display_url));
            results.push(OmniboxSuggestion {
                kind: SuggestionKind::SwitchTab,
                title: tab.title.clone(),
                subtitle: format!("Cambiar a pestaña · {}", tab.display_url),
                value: tab.display_url.clone(),
                tab_id: Some(tab.id),
            });
            if results.len() >= 4 {
                break;
            }
        }
    }

    // 3. Matching bookmarks
    for bm in bookmarks {
        if is_internal_url(&bm.url) {
            continue;
        }
        let key = norm_key(&bm.url);
        if seen_urls.contains(&key) {
            continue;
        }
        if bm.title.to_ascii_lowercase().contains(&ql) || bm.url.to_ascii_lowercase().contains(&ql)
        {
            seen_urls.insert(key);
            results.push(OmniboxSuggestion {
                kind: SuggestionKind::Bookmark,
                title: bm.title.clone(),
                subtitle: bm.url.clone(),
                value: bm.url.clone(),
                tab_id: None,
            });
            if results.len() >= 7 {
                break;
            }
        }
    }

    // 4. Matching history ranked by visit_count & recency
    let mut hist_matches: Vec<&HistoryEntry> = history
        .iter()
        .filter(|h| {
            !is_internal_url(&h.url)
                && !seen_urls.contains(&norm_key(&h.url))
                && (h.title.to_ascii_lowercase().contains(&ql)
                    || h.url.to_ascii_lowercase().contains(&ql))
        })
        .collect();
    hist_matches.sort_by(|a, b| {
        let a_starts = a
            .url
            .to_ascii_lowercase()
            .trim_start_matches("https://")
            .trim_start_matches("www.")
            .starts_with(&ql);
        let b_starts = b
            .url
            .to_ascii_lowercase()
            .trim_start_matches("https://")
            .trim_start_matches("www.")
            .starts_with(&ql);
        b_starts
            .cmp(&a_starts)
            .then_with(|| b.visit_count.cmp(&a.visit_count))
            .then_with(|| b.visited_at.cmp(&a.visited_at))
    });

    for h in hist_matches {
        let key = norm_key(&h.url);
        if seen_urls.insert(key) {
            results.push(OmniboxSuggestion {
                kind: SuggestionKind::History,
                title: h.title.clone(),
                subtitle: h.url.clone(),
                value: h.url.clone(),
                tab_id: None,
            });
            if results.len() >= 6 {
                break;
            }
        }
    }

    // 5. Optional live web search suggestions (DuckDuckGo privacy-respecting autocomplete)
    let engine_label = match engine {
        SearchEngine::DuckDuckGo => "DuckDuckGo",
        SearchEngine::Google => "Google",
        SearchEngine::Brave => "Brave Search",
        SearchEngine::Startpage => "Startpage",
        SearchEngine::Bing => "Bing",
    };
    for s in web_suggestions {
        let trimmed = s.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case(q) {
            continue;
        }
        if results
            .iter()
            .any(|r| r.value.eq_ignore_ascii_case(trimmed))
        {
            continue;
        }
        results.push(OmniboxSuggestion {
            kind: SuggestionKind::Search,
            title: trimmed.to_string(),
            subtitle: format!("Sugerencia web · {engine_label}"),
            value: trimmed.to_string(),
            tab_id: None,
        });
        if results.len() >= 9 {
            break;
        }
    }

    results
}

/// Fetches live search suggestions from DuckDuckGo's lightweight JSON endpoint.
pub fn fetch_web_suggestions(query: &str) -> Vec<String> {
    let q = query.trim();
    if q.len() < 2 || q.contains("://") || (!q.contains(' ') && q.contains('.')) {
        return Vec::new();
    }
    let url = format!(
        "https://duckduckgo.com/ac/?q={}&type=list",
        percent_encode_query(q)
    );
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(2200)))
        .build();
    let agent: ureq::Agent = config.into();
    let Ok(mut resp) = agent
        .get(&url)
        .header("User-Agent", "FerrumBrowser/0.4")
        .call()
    else {
        return Vec::new();
    };
    let Ok(text) = resp.body_mut().read_to_string() else {
        return Vec::new();
    };
    // DuckDuckGo `type=list` returns OpenSearch format: ["query", ["sug1", "sug2", ...]]
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    val.as_array()
        .and_then(|arr| arr.get(1))
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .take(5)
                .collect()
        })
        .unwrap_or_default()
}

fn norm_key(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}
