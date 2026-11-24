use std::fs;
use std::path::{Path, PathBuf};

use crate::storage::unix_now;

const MAX_CRASH_LOGS: usize = 10;

/// Installs a privacy-safe panic hook that persists diagnostic logs under
/// `<data_dir>/crashes/` without recording URLs or user secrets.
pub fn install_panic_hook(data_dir: PathBuf, console_echo: bool) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let raw_msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "panic".to_string()
        };
        let sanitized = sanitize_crash_text(&raw_msg);

        if console_echo {
            eprintln!("FERRUM_PANIC: {sanitized} @ {loc}");
        }

        let _ = write_crash_report(&data_dir, &sanitized, &loc);
        prev(info);
    }));
}

fn sanitize_crash_text(text: &str) -> String {
    // Strip any query string or fragment from accidental URL strings so tokens never leak into logs.
    text.split_whitespace()
        .map(|tok| {
            if tok.starts_with("http://") || tok.starts_with("https://") {
                tok.split(['?', '#']).next().unwrap_or(tok)
            } else {
                tok
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn write_crash_report(data_dir: &Path, message: &str, location: &str) -> std::io::Result<PathBuf> {
    let crashes_dir = data_dir.join("crashes");
    fs::create_dir_all(&crashes_dir)?;
    let ts = unix_now();
    let report = format!(
        "Ferrum Browser v{}\nOS: {} ({})\nTimestamp: {}\nLocation: {}\nMessage: {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        ts,
        location,
        message
    );
    let file_path = crashes_dir.join(format!("crash_{ts}_{}.log", std::process::id()));
    fs::write(&file_path, report)?;
    prune_old_crash_logs(&crashes_dir);
    Ok(file_path)
}

fn prune_old_crash_logs(crashes_dir: &Path) {
    let Ok(entries) = fs::read_dir(crashes_dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|x| x.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("log"))
        .collect();
    files.sort();
    if files.len() > MAX_CRASH_LOGS {
        let remove_count = files.len() - MAX_CRASH_LOGS;
        for old in files.into_iter().take(remove_count) {
            let _ = fs::remove_file(old);
        }
    }
}

/// Marks the browser session as currently active. Returns `true` if a previous run
/// exited uncleanly (crash or forced kill), enabling safe hibernated recovery.
pub fn mark_session_running(data_dir: &Path) -> bool {
    let marker = data_dir.join(".running");
    let crashed_previously = marker.exists();
    let _ = fs::create_dir_all(data_dir);
    let _ = fs::write(&marker, format!("{}:{}\n", std::process::id(), unix_now()));
    crashed_previously
}

/// Clears the active session marker on clean shutdown.
pub fn clear_session_running(data_dir: &Path) {
    let _ = fs::remove_file(data_dir.join(".running"));
}

/// Counts stored privacy-safe crash reports in `<data_dir>/crashes/`.
pub fn crash_reports_count(data_dir: &Path) -> usize {
    let crashes_dir = data_dir.join("crashes");
    fs::read_dir(crashes_dir)
        .ok()
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("log"))
                .count()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_marker_and_crash_report_lifecycle() {
        let base = std::env::temp_dir().join(format!(
            "ferrum_crash_test_{}_{}",
            unix_now(),
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        assert!(!mark_session_running(&base));
        assert!(mark_session_running(&base));
        clear_session_running(&base);
        assert!(!mark_session_running(&base));
        clear_session_running(&base);

        let cleaned = sanitize_crash_text("err at https://example.com/a?token=secret#frag ok");
        assert_eq!(cleaned, "err at https://example.com/a ok");

        let report = write_crash_report(&base, "test boom", "src/main.rs:1:1").unwrap();
        assert!(report.exists());
        assert_eq!(crash_reports_count(&base), 1);

        let _ = fs::remove_dir_all(&base);
    }
}
