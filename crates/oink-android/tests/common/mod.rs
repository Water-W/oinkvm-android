//! Shared by the host integration test and the browser preview example.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use oink_agent::api::{AgentApi, ApiEvent, SharingState, pairing};
use oink_agent::device_cards::DecimalU64;
use oink_agent::host::AgentHost;
use oink_agent::node::{Node, NodeOptions};
use oink_android::host::AndroidHost;
use oink_foundation::{Point, Rect, Size, display::DisplayInfo};
use oink_test_support::clipboard::FakeClipboard;
use oink_test_support::input::FakeInput;

pub fn wait(what: &str, secs: u64, ready: impl FnMut() -> bool) {
    wait_or(what, secs, ready, || {})
}
pub fn wait_or(what: &str, secs: u64, mut ready: impl FnMut() -> bool, explain: impl Fn()) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !ready() {
        if Instant::now() > deadline {
            explain();
            panic!("timed out waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn call(host: &AndroidHost, request: serde_json::Value) -> serde_json::Value {
    let reply: serde_json::Value = serde_json::from_str(&host.call(&request.to_string())).unwrap();
    assert!(reply.get("err").is_none(), "{request} failed: {reply}");
    reply["ok"].clone()
}

pub fn desktop(root: &std::path::Path, input: FakeInput, clipboard: Arc<FakeClipboard>) -> AgentHost {
    input.set_cursor(Point::new(800., 500.));
    AgentHost::start_with_fake_platform(
        NodeOptions {
            net: oink_test_support::network_config(root.join("desktop"), "desktop", 0),
            input: Arc::new(input),
            displays: vec![DisplayInfo {
                key: "main".into(),
                name: "main".into(),
                bounds: Rect::new(0., 0., 1600., 1000.),
                pixel_width: 1600,
                pixel_height: 1000,
                reference_extent: Some(Size::new(0.8, 0.5)),
                rotation_deg: 0,
                is_primary: true,
                is_builtin: false,
                is_virtual: false,
            }],
            display_backend: None,
            clipboard: Some(clipboard),
            mode_switch_in_callback: true,
            capture_reports_key_repeat: true,
            cursor_trail: false,
        },
        false,
    )
    .unwrap()
}

/// The desktop asks to pair with the phone; returns the pairing id.
pub fn request_pairing(desktop: &Node, phone: &AndroidHost) -> String {
    let phone_node = phone.node().unwrap();
    let (found_tx, found_rx) = std::sync::mpsc::channel();
    let operation = "00000000-0000-4000-8000-000000000001".to_string();
    let expected = operation.clone();
    let subscription = desktop.subscribe(Arc::new(move |event| {
        if let ApiEvent::AddressQueryFinished {
            operation_id, result, ..
        } = event
            && operation_id == &expected
        {
            let _ = found_tx.send(result.clone());
        }
    }));
    let port = phone_node.network_info().unwrap().port;
    let reply = desktop
        .query_pairing_address(operation, DecimalU64(1), "127.0.0.1".into(), Some(port))
        .unwrap();
    assert!(matches!(reply, pairing::PairReply::Value(_)), "{reply:?}");
    let found = found_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    desktop.unsubscribe(subscription);
    let pairing::AddressQueryResult::Found { peer } = found else {
        panic!("query failed: {found:?}")
    };
    let started = desktop
        .start_pairing(
            "00000000-0000-4000-8000-000000000002".into(),
            pairing::PairTarget::PreparedQuery {
                query_id: peer.query_id,
                generation: peer.generation,
            },
        )
        .unwrap();
    let pairing::PairReply::Value(pairing::Started::Started { pair_id }) = started else {
        panic!("start refused: {started:?}")
    };
    pair_id
}

/// Desktop initiates; the phone accepts and enters the code through JSON.
pub fn pair(desktop: &Node, phone: &AndroidHost) {
    let pair_id = request_pairing(desktop, phone);
    let phone_node = phone.node().unwrap();
    // The phone's interface sees the request through the JSON API.
    let mut request = serde_json::Value::Null;
    wait("incoming request on the phone", 10, || {
        let pairings = call(phone, serde_json::json!({"method": "pairings"}));
        request = pairings["requests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == pair_id.as_str())
            .cloned()
            .unwrap_or_default();
        !request.is_null()
    });
    let before = desktop
        .pairings()
        .unwrap()
        .sessions
        .into_iter()
        .find(|s| s.id == pair_id)
        .unwrap()
        .revision;
    call(
        phone,
        serde_json::json!({"method": "accept_pairing", "params": {
            "pair_id": pair_id, "expected_revision": request["revision"]
        }}),
    );
    wait("desktop shows the code", 10, || {
        desktop
            .pairings()
            .unwrap()
            .sessions
            .iter()
            .any(|s| s.id == pair_id && s.phase == 1 && s.revision > before)
    });
    let code = desktop.pairing_code(pair_id.clone()).unwrap().six_digits;
    let mut revision = serde_json::Value::Null;
    wait("phone ready for the code", 10, || {
        let pairings = call(phone, serde_json::json!({"method": "pairings"}));
        if let Some(session) = pairings["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == pair_id.as_str() && s["phase"] == 2)
        {
            revision = session["revision"].clone();
        }
        !revision.is_null()
    });
    call(
        phone,
        serde_json::json!({"method": "submit_pairing_code", "params": {
            "pair_id": pair_id, "expected_revision": revision, "attempt": 1, "code": code
        }}),
    );
    wait("paired and sharing on both sides", 20, || {
        let active = |a: &Node, b: &Node| {
            a.group()
                .unwrap()
                .members
                .iter()
                .any(|m| m.device.id.0 == b.id() && m.sharing == SharingState::Active)
        };
        active(desktop, &phone_node) && active(&phone_node, desktop)
    });
}
