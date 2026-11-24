use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use tao::event_loop::EventLoopProxy;

use crate::browser::BrowserEvent;

#[derive(Debug, Serialize, Deserialize)]
struct InstanceLock {
    port: u16,
    token: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HandOffPayload {
    token: String,
    urls: Vec<String>,
}

pub struct SingleInstanceGuard {
    lock_path: PathBuf,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

fn lock_file_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("FerrumBrowser");
    let _ = fs::create_dir_all(&dir);
    dir.join("instance.lock.json")
}

/// If another Ferrum main window is already running, forwards `urls` to it and returns `true`.
/// The caller should then exit cleanly instead of spawning a duplicate main window.
pub fn try_forward_to_running_instance(urls: &[String]) -> bool {
    let lock_path = lock_file_path();
    let Ok(raw) = fs::read_to_string(&lock_path) else {
        return false;
    };
    let Ok(lock) = serde_json::from_str::<InstanceLock>(&raw) else {
        return false;
    };

    let addr = format!("127.0.0.1:{}", lock.port);
    let Ok(mut stream) =
        TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(150))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(800)));

    let payload = HandOffPayload {
        token: lock.token,
        urls: urls.to_vec(),
    };
    let Ok(mut line) = serde_json::to_string(&payload) else {
        return false;
    };
    line.push('\n');

    if stream.write_all(line.as_bytes()).is_err() {
        return false;
    }

    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    matches!(reader.read_line(&mut reply), Ok(n) if n > 0 && reply.trim() == "OK")
}

/// Starts the localhost single-instance listener authenticated with a random 256-bit session token.
pub fn start_listener(proxy: EventLoopProxy<BrowserEvent>) -> Option<SingleInstanceGuard> {
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();

    let mut token_bytes = [0u8; 24];
    let _ = getrandom::fill(&mut token_bytes);
    let token = B64.encode(token_bytes);

    let lock_path = lock_file_path();
    let lock = InstanceLock {
        port,
        token: token.clone(),
    };
    if let Ok(json) = serde_json::to_string(&lock) {
        let _ = fs::write(&lock_path, json);
    }

    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));

            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            if reader.read_line(&mut line).is_ok()
                && let Ok(payload) = serde_json::from_str::<HandOffPayload>(line.trim())
                && payload.token == token
            {
                let _ = proxy.send_event(BrowserEvent::ExternalOpenUrls(payload.urls));
                let mut writer = stream;
                let _ = writer.write_all(b"OK\n");
            }
        }
    });

    Some(SingleInstanceGuard { lock_path })
}
