use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct DetectedBrowser {
    pub id: String,
    pub name: String,
    pub available: bool,
}

#[derive(Debug, Clone)]
pub struct ImportedBookmark {
    pub title: String,
    pub url: String,
}

pub fn detect_browsers() -> Vec<DetectedBrowser> {
    let sources = [
        ("chrome", "Google Chrome"),
        ("edge", "Microsoft Edge"),
        ("brave", "Brave Browser"),
    ];
    sources
        .iter()
        .map(|&(id, name)| DetectedBrowser {
            id: id.to_string(),
            name: name.to_string(),
            available: bookmarks_file_for(id).map(|p| p.exists()).unwrap_or(false),
        })
        .collect()
}

pub fn read_chromium_bookmarks(browser_id: &str) -> Result<Vec<ImportedBookmark>, String> {
    let path = bookmarks_file_for(browser_id)
        .ok_or_else(|| "Navegador no soportado para importación.".to_string())?;
    if !path.exists() {
        return Err(format!(
            "No se encontró el archivo de marcadores ({}).",
            path.display()
        ));
    }

    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("No se pudo leer el archivo de marcadores: {e}"))?;
    let root: Value =
        serde_json::from_str(&raw).map_err(|_| "Archivo Bookmarks inválido.".to_string())?;

    let mut out = Vec::new();
    if let Some(roots) = root.get("roots").and_then(|v| v.as_object()) {
        for node in roots.values() {
            collect_nodes(node, &mut out);
        }
    }
    Ok(out)
}

fn collect_nodes(node: &Value, out: &mut Vec<ImportedBookmark>) {
    if let Some(kind) = node.get("type").and_then(|v| v.as_str())
        && kind == "url"
        && let Some(url) = node.get("url").and_then(|v| v.as_str())
        && (url.starts_with("http://") || url.starts_with("https://"))
    {
        let title = node
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(url)
            .trim()
            .to_string();
        out.push(ImportedBookmark {
            title: if title.is_empty() {
                url.to_string()
            } else {
                title
            },
            url: url.to_string(),
        });
    }

    if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
        for child in children {
            collect_nodes(child, out);
        }
    }
}

fn bookmarks_file_for(browser_id: &str) -> Option<PathBuf> {
    let local = dirs::data_local_dir()?;
    match browser_id {
        "chrome" => Some(
            local
                .join("Google")
                .join("Chrome")
                .join("User Data")
                .join("Default")
                .join("Bookmarks"),
        ),
        "edge" => Some(
            local
                .join("Microsoft")
                .join("Edge")
                .join("User Data")
                .join("Default")
                .join("Bookmarks"),
        ),
        "brave" => Some(
            local
                .join("BraveSoftware")
                .join("Brave-Browser")
                .join("User Data")
                .join("Default")
                .join("Bookmarks"),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chromium_nested_bookmark_tree() {
        let sample = serde_json::json!({
            "roots": {
                "bookmark_bar": {
                    "type": "folder",
                    "children": [
                        { "type": "url", "name": "Rust", "url": "https://www.rust-lang.org/" },
                        {
                            "type": "folder",
                            "children": [
                                { "type": "url", "name": "Crates", "url": "https://crates.io/" }
                            ]
                        }
                    ]
                }
            }
        });
        let mut out = Vec::new();
        collect_nodes(&sample["roots"]["bookmark_bar"], &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].title, "Rust");
        assert_eq!(out[1].url, "https://crates.io/");
    }
}
