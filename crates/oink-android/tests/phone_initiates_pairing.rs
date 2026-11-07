#![cfg(debug_assertions)]
//! The phone initiates pairing with a desktop, then the phone backend restarts
//! on the same storage. Before the fix the phone had no layout-profile catalog:
//! the join failed with "pre-join catalog unavailable" and every later start
//! reported the group state unreadable. The cause was fractional dp screen
//! geometry, which the layout-profile catalog rejects.

mod common;

use std::sync::Arc;

use oink_android::host::{AndroidHost, StartConfig};
use oink_android::platform::{Button, Platform, Screen};
use oink_test_support::clipboard::FakeClipboard;
use oink_test_support::input::FakeInput;

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

#[test]
fn phone_initiated_pairing_completes_and_survives_restart() {
    oink_agent::logs::init();
    let temp = tempfile::tempdir().unwrap();
    let desktop = common::desktop(temp.path(), FakeInput::new(), Arc::new(FakeClipboard::new()));
    let net = oink_test_support::network_config(temp.path().join("phone"), "phone", 0);
    let start = || {
        AndroidHost::start(
            StartConfig {
                data_dir: temp.path().join("phone"),
                device_name: "phone".into(),
                screen: Screen::default(),
                network: Some(net.clone()),
            },
            Arc::new(Silent),
        )
        .expect("phone backend starts")
    };
    let phone = start();
    // The layout-profile catalog must exist before this device can initiate a join.
    let profiles = phone.call(r#"{"method":"layout_profiles"}"#);
    assert!(!profiles.contains("profile_error"), "{profiles}");
    common::pair_nodes(&phone.node().unwrap(), desktop.node());
    phone.stop();
    drop(phone);

    let phone = start();
    let overview: serde_json::Value = serde_json::from_str(&phone.call(r#"{"method":"overview"}"#)).unwrap();
    assert_eq!(overview["ok"]["group"]["member_count"], 2, "{overview}");
    phone.stop();
    desktop.shutdown_and_wait();
}
