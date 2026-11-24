use serde::Serialize;
use std::collections::HashSet;
use std::time::Duration;

use crate::calc::evaluate_omnibox_calc;
use crate::navigation::{SearchEngine, is_internal_url, match_search_bang, percent_encode_query};
use crate::storage::{Bookmark, CustomSearchEngine, HistoryEntry};
use crate::ui::TabSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKind {
    Calc,
    Bang,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn build_suggestions(
    raw_query: &str,
    tabs: &[TabSummary],
    active_tab_id: u64,
    bookmarks: &[Bookmark],
    history: &[HistoryEntry],
    engine: SearchEngine,
    custom_engines: &[CustomSearchEngine],
    web_suggestions: &[String],
) -> Vec<OmniboxSuggestion> {
    let q = raw_query.trim();
    if q.is_empty() {
        return Vec::new();
    }

    // Scoped filter prefixes: `@tabs`, `@bm`, `@hist`
    let (scope, filter_q) = parse_scope(q);
    let ql = filter_q.to_ascii_lowercase();
    let mut results = Vec::with_capacity(9);
    let mut seen_urls = HashSet::new();

    if scope.is_none() {
        // 0. Instant local calculator / unit converter
        if let Some(calc) = evaluate_omnibox_calc(q) {
            results.push(OmniboxSuggestion {
                kind: SuggestionKind::Calc,
                title: calc.display,
                subtitle: calc.subtitle,
                value: calc.copy_value,
                tab_id: None,
                badge: Some("Copiar".into()),
            });
        }

        // 0b. Built-in or custom Search Bang (`!yt`, `!gh`, `!mdn`, custom keywords)
        if let Some((bang_name, bang_query, bang_url)) = match_search_bang(q, custom_engines) {
            results.push(OmniboxSuggestion {
                kind: SuggestionKind::Bang,
                title: bang_query,
                subtitle: format!("Buscar directo en {bang_name}"),
                value: bang_url,
                tab_id: None,
                badge: Some(bang_name),
            });
        } else {
            // 1. Direct URL or search action as top row
            let looks_domain = !q.contains(' ') && (q.contains('.') || q.starts_with("localhost"));
            if looks_domain {
                results.push(OmniboxSuggestion {
                    kind: SuggestionKind::Navigate,
                    title: q.to_string(),
                    subtitle: "Ir directamente al sitio".to_string(),
                    value: q.to_string(),
                    tab_id: None,
                    badge: None,
                });
            } else {
                let engine_label = engine_display_name(engine);
                results.push(OmniboxSuggestion {
                    kind: SuggestionKind::Search,
                    title: q.to_string(),
                    subtitle: format!("Buscar en {engine_label}"),
                    value: q.to_string(),
                    tab_id: None,
                    badge: None,
                });
            }
        }
    }

    // 2. Open tabs ("Cambiar a esta pestaña")
    if matches!(scope, None | Some(Scope::Tabs)) {
        let limit = if scope == Some(Scope::Tabs) { 8 } else { 4 };
        for tab in tabs {
            if tab.id == active_tab_id && scope.is_none() {
                continue;
            }
            if tab.is_internal && scope.is_none() {
                continue;
            }
            let t_low = tab.title.to_ascii_lowercase();
            let u_low = tab.display_url.to_ascii_lowercase();
            if ql.is_empty() || t_low.contains(&ql) || u_low.contains(&ql) {
                seen_urls.insert(norm_key(&tab.display_url));
                results.push(OmniboxSuggestion {
                    kind: SuggestionKind::SwitchTab,
                    title: tab.title.clone(),
                    subtitle: format!(
                        "{}{}",
                        if tab.is_hibernated {
                            "Hibernada · "
                        } else {
                            ""
                        },
                        tab.display_url
                    ),
                    value: tab.display_url.clone(),
                    tab_id: Some(tab.id),
                    badge: Some("Cambiar".into()),
                });
                if results.len() >= limit {
                    break;
                }
            }
        }
    }

    // 3. Matching bookmarks
    if matches!(scope, None | Some(Scope::Bookmarks)) {
        let limit = if scope == Some(Scope::Bookmarks) {
            8
        } else {
            7
        };
        for bm in bookmarks {
            if is_internal_url(&bm.url) {
                continue;
            }
            let key = norm_key(&bm.url);
            if seen_urls.contains(&key) {
                continue;
            }
            if ql.is_empty()
                || bm.title.to_ascii_lowercase().contains(&ql)
                || bm.url.to_ascii_lowercase().contains(&ql)
            {
                seen_urls.insert(key);
                results.push(OmniboxSuggestion {
                    kind: SuggestionKind::Bookmark,
                    title: bm.title.clone(),
                    subtitle: bm.url.clone(),
                    value: bm.url.clone(),
                    tab_id: None,
                    badge: None,
                });
                if results.len() >= limit {
                    break;
                }
            }
        }
    }

    // 4. Matching history ranked by visit_count & recency
    if matches!(scope, None | Some(Scope::History)) {
        let limit = if scope == Some(Scope::History) { 8 } else { 6 };
        let mut hist_matches: Vec<&HistoryEntry> = history
            .iter()
            .filter(|h| {
                !is_internal_url(&h.url)
                    && !seen_urls.contains(&norm_key(&h.url))
                    && (ql.is_empty()
                        || h.title.to_ascii_lowercase().contains(&ql)
                        || h.url.to_ascii_lowercase().contains(&ql))
            })
            .collect();
        hist_matches.sort_by(|a, b| {
            let a_starts = !ql.is_empty()
                && a.url
                    .to_ascii_lowercase()
                    .trim_start_matches("https://")
                    .trim_start_matches("www.")
                    .starts_with(&ql);
            let b_starts = !ql.is_empty()
                && b.url
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
                    badge: None,
                });
                if results.len() >= limit {
                    break;
                }
            }
        }
    }

    // 5. Optional live web search suggestions (DuckDuckGo privacy-respecting autocomplete)
    if scope.is_none() {
        let engine_label = engine_display_name(engine);
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
                badge: None,
            });
            if results.len() >= 9 {
                break;
            }
        }
    }

    results
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Tabs,
    Bookmarks,
    History,
}

fn parse_scope(q: &str) -> (Option<Scope>, &str) {
    let lower = q.to_ascii_lowercase();
    for (prefix, sc) in [
        ("@tabs", Scope::Tabs),
        ("@tab", Scope::Tabs),
        ("@t", Scope::Tabs),
        ("@bookmarks", Scope::Bookmarks),
        ("@bm", Scope::Bookmarks),
        ("@b", Scope::Bookmarks),
        ("@history", Scope::History),
        ("@hist", Scope::History),
        ("@h", Scope::History),
    ] {
        if lower == prefix {
            return (Some(sc), "");
        }
        if let Some(rest) = lower.strip_prefix(prefix)
            && rest.starts_with(' ')
        {
            return (Some(sc), q[prefix.len()..].trim());
        }
    }
    (None, q)
}

fn engine_display_name(engine: SearchEngine) -> &'static str {
    match engine {
        SearchEngine::DuckDuckGo => "DuckDuckGo",
        SearchEngine::Google => "Google",
        SearchEngine::Brave => "Brave Search",
        SearchEngine::Startpage => "Startpage",
        SearchEngine::Bing => "Bing",
    }
}

/// Fetches live search suggestions from DuckDuckGo's lightweight JSON endpoint.
pub fn fetch_web_suggestions(query: &str) -> Vec<String> {
    let q = query.trim();
    if q.len() < 2
        || q.starts_with('@')
        || q.starts_with('=')
        || q.starts_with('!')
        || q.contains("://")
        || (!q.contains(' ') && q.contains('.'))
    {
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
        .header("User-Agent", "FerrumBrowser/0.6")
        .call()
    else {
        return Vec::new();
    };
    let Ok(text) = resp.body_mut().read_to_string() else {
        return Vec::new();
    };
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
