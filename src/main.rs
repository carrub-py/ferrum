#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adblock;
mod autocomplete;
mod backup;
mod browser;
mod calc;
mod crash;
mod extensions;
mod importer;
mod navigation;
mod os_integration;
mod single_instance;
mod storage;
mod sync;
mod ui;
mod updater;
mod vault;
mod webview_hooks;

use std::time::{Duration, Instant};

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Icon, Theme as WindowTheme, WindowBuilder};

#[cfg(target_os = "windows")]
use tao::platform::windows::WindowBuilderExtWindows;

use crate::browser::{AppLaunchMode, BrowserApp, BrowserEvent, ShortcutAction};
use crate::navigation::resolve_cli_target;

const FERRUM_ICON_RGBA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ferrum_icon_64.rgba"));

fn main() -> wry::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let smoke_test = args.iter().any(|a| a == "--smoke-test");
    let stress_test = args.iter().any(|a| a == "--stress-test");
    let new_window = args.iter().any(|a| a == "--new-window");
    let post_update = args.iter().any(|a| a == "--post-update-wait");

    if post_update {
        std::thread::sleep(Duration::from_millis(700));
    }
    if !smoke_test && !stress_test {
        updater::cleanup_old_binary_on_startup();
    }

    let crash_data_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("FerrumBrowser");
    crash::install_panic_hook(crash_data_dir, smoke_test || stress_test);

    let app_url = args
        .iter()
        .find_map(|a| a.strip_prefix("--app="))
        .map(|s| s.trim_matches('"').to_string())
        .filter(|s| !s.is_empty());
    let app_title = args
        .iter()
        .find_map(|a| a.strip_prefix("--app-title="))
        .map(|s| s.trim_matches('"').to_string())
        .filter(|s| !s.is_empty());

    let cli_urls: Vec<String> = args
        .iter()
        .filter_map(|arg| resolve_cli_target(arg, true, true))
        .collect();

    // Single-Instance IPC handoff: if another Ferrum main window is already running,
    // forward CLI URLs/files to it as new tabs and focus it, then exit cleanly!
    if !smoke_test
        && !stress_test
        && !new_window
        && app_url.is_none()
        && single_instance::try_forward_to_running_instance(&cli_urls)
    {
        return Ok(());
    }

    let event_loop = EventLoopBuilder::<BrowserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let _single_instance_guard = if !smoke_test && !stress_test && !new_window && app_url.is_none()
    {
        single_instance::start_listener(proxy.clone())
    } else {
        None
    };

    let window_icon = Icon::from_rgba(FERRUM_ICON_RGBA.to_vec(), 64, 64).ok();

    let (init_w, init_h, init_title) = if app_url.is_some() {
        (
            1160.0,
            780.0,
            app_title.as_deref().unwrap_or("Ferrum App").to_string(),
        )
    } else {
        (1380.0, 880.0, "Ferrum Browser".to_string())
    };

    let builder = WindowBuilder::new()
        .with_title(init_title)
        .with_window_icon(window_icon)
        .with_inner_size(LogicalSize::new(init_w, init_h))
        .with_min_inner_size(LogicalSize::new(640.0, 440.0))
        .with_decorations(false)
        .with_background_color((0, 0, 0, 255))
        .with_visible(false)
        .with_theme(Some(WindowTheme::Dark));

    #[cfg(target_os = "windows")]
    let builder = builder.with_undecorated_shadow(true);

    let window = builder
        .build(&event_loop)
        .expect("No se pudo crear la ventana principal de Ferrum");

    webview_hooks::apply_windows11_frame_styling(&window);

    let launch_mode = if let Some(url) = app_url {
        let resolved = resolve_cli_target(&url, true, true).unwrap_or(url);
        AppLaunchMode::Pwa {
            url: resolved,
            title: app_title,
        }
    } else {
        AppLaunchMode::Normal {
            restore_session: !smoke_test && !stress_test && !new_window,
            cli_urls,
            smoke_test,
            ephemeral_test: smoke_test || stress_test,
        }
    };

    // Background timers use safe buffered UserEvents instead of ControlFlow::WaitUntil,
    // which prevents re-entrant tao::EventLoopRunner::poll() panics during WebView2 creation.
    if smoke_test {
        let wd = proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(15));
            let _ = wd.send_event(BrowserEvent::TestTimeout);
        });
    } else if stress_test {
        let wd = proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(30));
            let _ = wd.send_event(BrowserEvent::TestTimeout);
        });
        let driver = proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            // Burst 1: 8 rapid tabs (30ms apart)
            for _ in 0..8 {
                let _ = driver.send_event(BrowserEvent::Ipc(ui::IpcCommand::NewTab {
                    url: None,
                    private: Some(false),
                }));
                std::thread::sleep(Duration::from_millis(30));
            }
            // Pause long enough for debounced WarmSpareTab (1200ms) to trigger mid-session
            std::thread::sleep(Duration::from_millis(1350));
            // Burst 2: 7 mixed-pace tabs (including 1 private tab) -> 16 tabs open simultaneously
            for i in 0..7 {
                let _ = driver.send_event(BrowserEvent::Ipc(ui::IpcCommand::NewTab {
                    url: None,
                    private: Some(i == 3),
                }));
                let gap = if i % 2 == 0 { 45 } else { 320 };
                std::thread::sleep(Duration::from_millis(gap));
            }
            // Exercise periodic maintenance + tab switching
            let _ = driver.send_event(BrowserEvent::PeriodicTick);
            let _ = driver.send_event(BrowserEvent::Shortcut(ShortcutAction::NextTab));
            let _ = driver.send_event(BrowserEvent::Shortcut(ShortcutAction::PrevTab));
            let _ = driver.send_event(BrowserEvent::Ipc(ui::IpcCommand::SwitchTab { id: 1 }));
            std::thread::sleep(Duration::from_millis(80));
            // Rapidly close 10 tabs
            for _ in 0..10 {
                let _ = driver.send_event(BrowserEvent::Shortcut(ShortcutAction::CloseTab));
                std::thread::sleep(Duration::from_millis(25));
            }
            // Wait for spare tab replenishment again and open 3 more tabs -> expected 9 tabs total
            std::thread::sleep(Duration::from_millis(1350));
            for _ in 0..3 {
                let _ = driver.send_event(BrowserEvent::Ipc(ui::IpcCommand::NewTab {
                    url: None,
                    private: Some(false),
                }));
                std::thread::sleep(Duration::from_millis(60));
            }
            std::thread::sleep(Duration::from_millis(300));
            let _ = driver.send_event(BrowserEvent::RequestExit);
        });
    } else {
        let tick_proxy = proxy.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(30));
                if tick_proxy.send_event(BrowserEvent::PeriodicTick).is_err() {
                    break;
                }
            }
        });

        let upd_proxy = proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            let _ = upd_proxy.send_event(BrowserEvent::StartupUpdateCheck);
        });
    }

    let mut app = BrowserApp::new(window, proxy, launch_mode)?;
    if post_update {
        app.notify_post_update();
    }
    app.flush_ui_if_dirty();
    if !smoke_test && !stress_test {
        app.reveal_window();
    }

    let start = Instant::now();
    let mut ui_ready_count: usize = 0;
    let mut page_finished = false;

    event_loop.run(move |event, _, control_flow| {
        // Always use ControlFlow::Wait so tao never posts PROCESS_NEW_EVENTS_MSG_ID
        // re-entrantly while wry::WebViewBuilder pumps Win32 messages via wait_with_pump.
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                app.persist_on_exit();
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                if size.width > 0 && size.height > 0 {
                    app.handle_resize(Some((size.width, size.height)));
                }
            }
            Event::WindowEvent {
                event: WindowEvent::ScaleFactorChanged { new_inner_size, .. },
                ..
            } => {
                if new_inner_size.width > 0 && new_inner_size.height > 0 {
                    app.handle_resize(Some((new_inner_size.width, new_inner_size.height)));
                }
            }
            Event::WindowEvent {
                event: WindowEvent::Moved(_),
                ..
            } => {
                app.handle_window_moved();
            }
            Event::WindowEvent {
                event: WindowEvent::Focused(true),
                ..
            } => {
                app.handle_window_focused();
            }
            Event::UserEvent(BrowserEvent::RequestExit) => {
                if stress_test {
                    let count = app.tabs_count();
                    let active = app.active_tab_id();
                    let elapsed = start.elapsed().as_millis();
                    app.persist_on_exit();
                    if count == 9 && active > 0 {
                        println!("STRESS_OK: tabs={count} active_id={active} elapsed_ms={elapsed}");
                        *control_flow = ControlFlow::Exit;
                    } else {
                        eprintln!(
                            "STRESS_MISMATCH: expected=9 tabs={count} active_id={active} elapsed_ms={elapsed}"
                        );
                        *control_flow = ControlFlow::ExitWithCode(1);
                    }
                } else {
                    app.persist_on_exit();
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(BrowserEvent::TestTimeout) => {
                if smoke_test {
                    eprintln!(
                        "SMOKE_TIMEOUT: tabs={} ui_ready={} page_finished={}",
                        app.tabs_count(),
                        ui_ready_count,
                        page_finished
                    );
                    *control_flow = ControlFlow::ExitWithCode(1);
                } else if stress_test {
                    eprintln!(
                        "STRESS_TIMEOUT: tabs={} elapsed_ms={}",
                        app.tabs_count(),
                        start.elapsed().as_millis()
                    );
                    *control_flow = ControlFlow::ExitWithCode(1);
                }
            }
            Event::UserEvent(user_event) => {
                if smoke_test {
                    match &user_event {
                        BrowserEvent::Ipc(ui::IpcCommand::UiReady) => ui_ready_count += 1,
                        BrowserEvent::TabLoadEvent { started: false, .. } => page_finished = true,
                        _ => {}
                    }
                }
                app.handle_event(user_event);
            }
            Event::MainEventsCleared => {
                // Only non-blocking UI & session flushing runs here — never WebView creation!
                app.flush_ui_if_dirty();

                if smoke_test && ui_ready_count >= 3 && page_finished {
                    println!(
                        "SMOKE_OK: tabs={} ui_ready={} page_finished={}",
                        app.tabs_count(),
                        ui_ready_count,
                        page_finished
                    );
                    app.persist_on_exit();
                    *control_flow = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    });
}
