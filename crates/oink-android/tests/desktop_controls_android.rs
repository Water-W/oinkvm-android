#![cfg(debug_assertions)]
//! A desktop backend (fake OS input) and the Android host on loopback:
//! pairing through the Android JSON API, then the desktop pointer and
//! keyboard crossing onto the Android screen, and clipboard text both ways.
//! Real nodes, QUIC/TLS and routing; only the OS layers are synthetic.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{desktop, pair, wait, wait_or};
use oink_agent::api::AgentApi;
use oink_android::host::{AndroidHost, StartConfig};
use oink_android::platform::{Button, Platform, Screen};
use oink_clipboard::ClipboardBackend;
use oink_foundation::input::{CaptureMode, CapturedEvent, Disposition, InputEvent, SourceKind};
use oink_foundation::{Point, Rect};
use oink_input::InputBackend;
use oink_test_support::clipboard::FakeClipboard;
use oink_test_support::input::FakeInput;

#[derive(Default)]
struct Phone {
    log: Mutex<Vec<String>>,
    clipboard: Mutex<Vec<String>>,
}
impl Phone {
    fn has(&self, prefix: &str) -> bool {
        self.log.lock().unwrap().iter().any(|line| line.starts_with(prefix))
    }
}
impl Platform for Phone {
    fn pointer(&self, x: f64, y: f64) {
        self.log.lock().unwrap().push(format!("pointer {x:.0} {y:.0}"));
    }
    fn button(&self, button: Button, pressed: bool, _x: f64, _y: f64) {
        self.log.lock().unwrap().push(format!("button {button:?} {pressed}"));
    }
    fn scroll(&self, _x: f64, _y: f64, dx: f64, dy: f64) {
        self.log.lock().unwrap().push(format!("scroll {dx:.0} {dy:.0}"));
    }
    fn key(&self, usage: u32, pressed: bool, _modifiers: u8, text: Option<&str>) {
        self.log
            .lock()
            .unwrap()
            .push(format!("key {usage:#x} {pressed} {text:?}"));
    }
    fn pointer_hidden(&self) {}
    fn set_clipboard(&self, text: &str) {
        self.clipboard.lock().unwrap().push(text.into());
    }
    fn event(&self, _json: &str) {}
}

fn feed_move(input: &FakeInput, dx: f64) {
    let bounds = Rect::new(0.0, 0.0, 1600.0, 1000.0);
    let cur = input.cursor_position().unwrap();
    let local = matches!(input.mode(), Some(CaptureMode::Local));
    let next = if local {
        bounds.clamp(Point::new(cur.x + dx, cur.y))
    } else {
        cur
    };
    let d = input.feed(&CapturedEvent {
        physical: true,
        event: InputEvent::PointerDelta { dx, dy: 0.0 },
        source: SourceKind::Mouse,
        time_us: 0,
        cursor: Some(next),
    });
    if local && d == Disposition::Pass {
        input.set_cursor(next);
    }
}

fn feed(input: &FakeInput, event: InputEvent) {
    input.feed(&CapturedEvent {
        physical: true,
        event,
        source: SourceKind::Keyboard,
        time_us: 0,
        cursor: None,
    });
}

#[test]
fn desktop_pointer_keyboard_and_clipboard_reach_android() {
    oink_agent::logs::init();
    let temp = tempfile::tempdir().unwrap();
    let input = FakeInput::new();
    let desktop_clipboard = Arc::new(FakeClipboard::new());
    let desktop = desktop(temp.path(), input.clone(), desktop_clipboard.clone());
    let desktop_node = desktop.node().clone();
    let phone_platform = Arc::new(Phone::default());
    let phone = AndroidHost::start(
        StartConfig {
            data_dir: temp.path().join("phone"),
            device_name: "phone".into(),
            screen: Screen {
                width_px: 1080,
                height_px: 2400,
                density: 2.7,
                xdpi: 400.0,
                ydpi: 400.0,
                rotation_deg: 0,
            },
            network: Some(oink_test_support::network_config(temp.path().join("phone"), "phone", 0)),
        },
        phone_platform.clone(),
    )
    .unwrap();
    phone.set_accessibility(true);
    pair(&desktop_node, &phone);

    // The phone appears in the shared layout as one dp-sized screen.
    let phone_id = phone.node().unwrap().id();
    let mut screens = Vec::new();
    wait("phone screen in the desktop layout", 10, || {
        screens = desktop_node.layout().0.screens;
        screens.iter().any(|s| s.id.device == phone_id)
    });
    let desktop_x = screens
        .iter()
        .find(|s| s.id.device == desktop_node.id())
        .unwrap()
        .rect
        .x;
    let phone_x = screens.iter().find(|s| s.id.device == phone_id).unwrap().rect.x;
    let toward = if phone_x > desktop_x { 1.0 } else { -1.0 };

    // Push the desktop pointer over the shared edge.
    wait("desktop online with the phone", 10, || {
        desktop_node.online_set().contains(&phone_id)
    });
    // The phone is shorter than the desktop: cross where both edges overlap.
    input.set_cursor(Point::new(if toward > 0.0 { 1580.0 } else { 20.0 }, 100.0));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !phone_platform.has("pointer ") {
        if Instant::now() > deadline {
            eprintln!("screens: {:#?}", desktop_node.layout().0.screens);
            eprintln!("desktop input: {:#?}", desktop_node.input_status());
            eprintln!("phone overview: {:#?}", phone.node().unwrap().overview());
            eprintln!("phone cards: {}", phone.call(r#"{"method":"device_cards"}"#));
            eprintln!("cursor {:?} mode {:?}", input.cursor_position(), input.mode());
            panic!("pointer never reached the phone");
        }
        feed_move(&input, 10.0 * toward);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(matches!(input.mode(), Some(CaptureMode::Remote { .. })));

    // Click and type while the phone holds focus.
    feed(
        &input,
        InputEvent::Button {
            button: oink_foundation::input::MouseButton::Left,
            pressed: true,
        },
    );
    feed(
        &input,
        InputEvent::Button {
            button: oink_foundation::input::MouseButton::Left,
            pressed: false,
        },
    );
    let a = oink_foundation::input::KeyCode::keyboard(0x04);
    feed(&input, InputEvent::Key { code: a, pressed: true });
    feed(
        &input,
        InputEvent::Key {
            code: a,
            pressed: false,
        },
    );
    wait("click and text on the phone", 10, || {
        phone_platform.has("button Primary false") && phone_platform.has("key 0x4 true Some(\"a\")")
    });

    // Clipboard: desktop → phone.
    desktop_clipboard.copy(
        vec![oink_clipboard::ClipData::Text("copied on the desktop".into())],
        oink_clipboard::Sensitivity::Normal,
    );
    wait_or(
        "desktop text on the phone clipboard",
        15,
        || {
            phone_platform
                .clipboard
                .lock()
                .unwrap()
                .iter()
                .any(|t| t == "copied on the desktop")
        },
        || {
            eprintln!("desktop activity {:?}", desktop_node.clipboard_activity());
            eprintln!("phone activity {}", phone.call(r#"{"method":"clipboard_activity"}"#));
            eprintln!("phone transfer {}", phone.call(r#"{"method":"clipboard_transfer"}"#));
            eprintln!(
                "phone clip settings {}",
                phone.call(r#"{"method":"clipboard_settings"}"#)
            );
        },
    );
    // Phone → desktop.
    phone.clipboard_text(Some("copied on the phone".into()));
    wait("phone text on the desktop clipboard", 15, || {
        desktop_clipboard
            .read()
            .unwrap()
            .items
            .iter()
            .any(|item| matches!(item, oink_clipboard::ClipData::Text(t) if t == "copied on the phone"))
    });

    phone.stop();
    desktop.shutdown_and_wait();
}
