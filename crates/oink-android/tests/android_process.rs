//! The JNI start sequence in an environment like an Android app process:
//! no `HOME` and an unwritable `TMPDIR`. Before `prepare_process`, resolving the
//! log directory panicked (`HOME is required`) and the panic aborted the app.
//! Runs without the test harness so the environment changes precede any thread.

use std::sync::Arc;

use oink_android::host::{AndroidHost, StartConfig, prepare_process};
use oink_android::platform::{Button, Platform, Screen};

struct Silent;
impl Platform for Silent {
    fn pointer(&self, _: f64, _: f64) {}
    fn button(&self, _: Button, _: bool, _: f64, _: f64) {}
    fn scroll(&self, _: f64, _: f64, _: f64, _: f64) {}
    fn key(&self, _: u32, _: bool, _: u8, _: Option<&str>) {}
    fn pointer_hidden(&self) {}
    fn set_clipboard(&self, _: &str) {}
    fn event(&self, _: &str) {}
}

fn main() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files").join("oink");
    // SAFETY: single-threaded so far (harness = false).
    unsafe {
        std::env::remove_var("HOME");
        std::env::remove_var("OINK_HOME");
        std::env::remove_var("XDG_DATA_HOME");
        std::env::set_var("TMPDIR", "/proc/oink-unwritable");
        prepare_process(&root, "zh-CN").unwrap();
    }
    assert_eq!(oink_foundation::paths::data_dir(), root);
    assert_eq!(oink_foundation::paths::log_dir(), root.join("Logs"));
    assert!(std::env::temp_dir().starts_with(&root));
    tempfile::tempfile().expect("temporary files work");

    let host = AndroidHost::start(
        StartConfig {
            data_dir: root.clone(),
            device_name: "phone".into(),
            screen: Screen::default(),
            network: Some(oink_test_support::network_config(root.clone(), "phone", 0)),
        },
        Arc::new(Silent),
    )
    .expect("backend starts without HOME");
    let reply: serde_json::Value = serde_json::from_str(&host.call(r#"{"method":"overview"}"#)).unwrap();
    assert!(reply.get("ok").is_some(), "{reply}");
    host.stop();
    assert!(root.join("Logs").is_dir());
    println!("android_process: ok");
}
