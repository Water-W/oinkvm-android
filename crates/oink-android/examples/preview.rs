//! Browser preview of the Android app's interface against a real backend.
//!
//! Starts the Android host (synthetic platform layer) and, with `--paired`, a
//! second desktop backend paired to it on loopback (with `--incoming`, the
//! desktop instead asks to pair a few seconds after start, so the phone's
//! interface receives a request), then serves the APK's web
//! assets (`apps/android/app/src/main/assets/web`, produced by
//! `apps/android/build.py --skip-rust --skip-apk`) on http://127.0.0.1:1450.
//! `preview.js` stands in for the Android `OinkAndroid` JavaScript interface
//! and forwards it over HTTP, so the interface runs through the same
//! `oink-android.js` shim as on a device.
//!
//!     cargo run -p oink-android --example preview -- --paired
//!
//! With a desktop backend, its interface is served on the next port (1451).
//!
//! Development only: no authentication; it binds to loopback.

#[path = "../tests/common/mod.rs"]
mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

use oink_android::host::{AndroidHost, StartConfig};
use oink_android::platform::{Button, Platform, Screen};

const PORT: u16 = 1450;

/// Logs injected input; queues interface events for `/events`.
#[derive(Default)]
struct PreviewPlatform {
    subscribers: Subscribers,
}

type Subscribers = Arc<Mutex<Vec<mpsc::Sender<String>>>>;

/// One backend whose interface is served: the phone, or the paired desktop.
struct Served {
    call: Box<dyn Fn(&str) -> String + Send + Sync>,
    live_views: Box<dyn Fn(&str) + Send + Sync>,
    subscribers: Subscribers,
    platform: &'static str,
}

impl Platform for PreviewPlatform {
    fn pointer(&self, _x: f64, _y: f64) {}
    fn button(&self, button: Button, pressed: bool, x: f64, y: f64) {
        println!("button {button:?} {pressed} at {x:.0},{y:.0}");
    }
    fn scroll(&self, _x: f64, _y: f64, dx: f64, dy: f64) {
        println!("scroll {dx:.0},{dy:.0}");
    }
    fn key(&self, usage: u32, pressed: bool, _modifiers: u8, text: Option<&str>) {
        println!("key {usage:#x} {pressed} {text:?}");
    }
    fn pointer_hidden(&self) {}
    fn set_clipboard(&self, text: &str) {
        println!("clipboard {text:?}");
    }
    fn event(&self, json: &str) {
        self.subscribers
            .lock()
            .unwrap()
            .retain(|subscriber| subscriber.send(json.to_owned()).is_ok());
    }
}

const BRIDGE: &str = r#"
// Stands in for the Android JavascriptInterface: invoke(id, command, argsJson).
window.OinkAndroid = {
  invoke(id, command, args) {
    fetch('/invoke', { method: 'POST', body: JSON.stringify({ command, args: JSON.parse(args) }) })
      .then(r => r.text())
      .then(text => window.__oinkAndroid.reply(id, text));
  },
};
const events = new EventSource('/events');
events.onmessage = m => window.__oinkAndroid && window.__oinkAndroid.event(JSON.parse(m.data));
"#;

fn main() -> anyhow::Result<()> {
    oink_agent::logs::init();
    let paired = std::env::args().any(|a| a == "--paired");
    let incoming = std::env::args().any(|a| a == "--incoming");
    let web = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/android/app/src/main/assets/web");
    anyhow::ensure!(
        web.join("index.html").is_file(),
        "run `python apps/android/build.py --skip-rust --skip-apk` first"
    );
    let temp = tempfile::tempdir()?;
    let platform = Arc::new(PreviewPlatform::default());
    let phone = Arc::new(AndroidHost::start(
        StartConfig {
            data_dir: temp.path().join("phone"),
            device_name: "Pixel 9".into(),
            screen: Screen {
                width_px: 1080,
                height_px: 2424,
                density: 2.625,
                xdpi: 422.0,
                ydpi: 422.0,
                rotation_deg: 0,
            },
            network: Some(oink_test_support::network_config(
                temp.path().join("phone"),
                "Pixel 9",
                0,
            )),
        },
        platform.clone(),
    )?);
    // `--no-accessibility`: as if the accessibility service were turned off.
    phone.set_accessibility(!std::env::args().any(|a| a == "--no-accessibility"));
    let _desktop = if paired {
        let desktop = common::desktop(
            temp.path(),
            oink_test_support::input::FakeInput::new(),
            Arc::new(oink_test_support::clipboard::FakeClipboard::new()),
        );
        common::pair(desktop.node(), &phone);
        println!("paired with a desktop backend");
        Some(desktop)
    } else if incoming {
        let desktop = common::desktop(
            temp.path(),
            oink_test_support::input::FakeInput::new(),
            Arc::new(oink_test_support::clipboard::FakeClipboard::new()),
        );
        let (node, target) = (desktop.node().clone(), phone.clone());
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(8));
            let id = common::request_pairing(&node, &target);
            println!("desktop requested pairing {id}");
        });
        Some(desktop)
    } else {
        None
    };
    let served_phone = {
        let (call, live) = (phone.clone(), phone.clone());
        Arc::new(Served {
            call: Box::new(move |request| call.call(request)),
            live_views: Box::new(move |views| live.set_live_views(views)),
            subscribers: platform.subscribers.clone(),
            platform: "linux",
        })
    };
    // The paired desktop's own interface, to view the group from its side.
    if let Some(desktop) = &_desktop {
        let node = desktop.node().clone();
        let subscribers: Subscribers = Arc::default();
        let events = subscribers.clone();
        node.subscribe(Arc::new(move |event| {
            if let Ok(json) = serde_json::to_string(event) {
                events.lock().unwrap().retain(|s| s.send(json.clone()).is_ok());
            }
        }));
        let served = Arc::new(Served {
            call: Box::new(move |request| {
                let result = serde_json::from_str::<oink_agent::api::Request>(request)
                    .map_err(|e| oink_agent::api::ApiError::InvalidArgument { message: e.to_string() })
                    .and_then(|request| oink_agent::api::dispatch(&node, request));
                match result {
                    Ok(reply) => serde_json::json!({ "ok": reply }).to_string(),
                    Err(error) => serde_json::json!({ "err": error }).to_string(),
                }
            }),
            live_views: Box::new(|_| {}),
            subscribers,
            platform: "windows",
        });
        listen(PORT + 1, served, web.clone())?;
        println!("desktop: http://127.0.0.1:{}/index.html?view=main", PORT + 1);
    }
    println!("preview: http://127.0.0.1:{PORT}/index.html?view=main");
    listen(PORT, served_phone, web)?.join().ok();
    Ok(())
}

fn listen(port: u16, served: Arc<Served>, web: PathBuf) -> std::io::Result<std::thread::JoinHandle<()>> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    Ok(std::thread::spawn(move || {
        for stream in listener.incoming() {
            let (Ok(stream), served, web) = (stream, served.clone(), web.clone()) else {
                continue;
            };
            std::thread::spawn(move || {
                let _ = serve(stream, &served, &web);
            });
        }
    }))
}

fn serve(mut stream: TcpStream, served: &Served, web: &Path) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let mut length = 0;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        if header.trim().is_empty() {
            break;
        }
        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let path = target.split('?').next().unwrap_or("/");
    match (method, path) {
        ("POST", "/invoke") => {
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let reply = invoke(served, &value["command"], &value["args"]);
            respond(&mut stream, "application/json", reply.as_bytes())
        }
        ("GET", "/events") => {
            let (send, receive) = mpsc::channel();
            served.subscribers.lock().unwrap().push(send);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\n\r\n")?;
            for json in receive {
                stream.write_all(format!("data: {json}\n\n").as_bytes())?;
            }
            Ok(())
        }
        ("GET", path) => {
            let name = path.trim_start_matches('/');
            let name = if name.is_empty() { "index.html" } else { name };
            if name.contains("..") {
                return respond(&mut stream, "text/plain", b"");
            }
            let Ok(mut bytes) = std::fs::read(web.join(name)) else {
                stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")?;
                return Ok(());
            };
            if name == "index.html" {
                // Install the stand-in interface before the shim reads it.
                let html =
                    String::from_utf8_lossy(&bytes).replace("<head>", &format!("<head><script>{BRIDGE}</script>"));
                bytes = html.into_bytes();
            }
            let mime = match name.rsplit('.').next() {
                Some("html") => "text/html",
                Some("js") => "text/javascript",
                Some("css") => "text/css",
                Some("png") => "image/png",
                Some("svg") => "image/svg+xml",
                _ => "application/octet-stream",
            };
            respond(&mut stream, mime, &bytes)
        }
        _ => respond(&mut stream, "text/plain", b""),
    }
}

/// The same command mapping as `MainActivity.Bridge` / `MainActivity.shell`.
fn invoke(served: &Served, command: &serde_json::Value, args: &serde_json::Value) -> String {
    let ok = |value: serde_json::Value| serde_json::json!({ "ok": value }).to_string();
    match command.as_str().unwrap_or("") {
        "api_call" => (served.call)(&args["request"].to_string()),
        "boot_info" => ok(serde_json::json!({
            "backend": "agent", "platform": served.platform, "version": env!("CARGO_PKG_VERSION"),
            "build": null, "open_update_failure": false
        })),
        "live_views" => {
            (served.live_views)(&args["views"].to_string());
            ok(serde_json::Value::Null)
        }
        _ => ok(serde_json::Value::Null),
    }
}

fn respond(stream: &mut TcpStream, mime: &str, body: &[u8]) -> std::io::Result<()> {
    stream.write_all(
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: {mime}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    )?;
    stream.write_all(body)
}
