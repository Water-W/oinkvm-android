//! One embedded backend per process, driven by JSON requests from the app.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use oink_agent::api::{AgentApi, ApiError, ApiEvent, Request, SubscriptionId};
use oink_agent::host::AgentHost;
use oink_agent::node::NodeOptions;
use oink_network::NetworkConfig;
use serde::Deserialize;

use crate::clipboard::AndroidClipboard;
use crate::display::AndroidDisplay;
use crate::input::AndroidInput;
use crate::platform::{Platform, Screen, Shared};

/// What the app knows at service start.
#[derive(Clone, Debug)]
pub struct StartConfig {
    /// App-private storage; becomes the backend's data directory.
    pub data_dir: PathBuf,
    /// Initial device name (the user can rename it later).
    pub device_name: String,
    pub screen: Screen,
    /// Overrides for isolated tests; production uses the default ports and discovery.
    pub network: Option<NetworkConfig>,
}

/// Live measurement events the interface asked for (`live_views` shell command).
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default)]
pub struct LiveViews {
    pub stats: bool,
    pub card_signals: bool,
    pub group_quality: bool,
    pub audio_quality: bool,
    pub diagnostics: bool,
}

impl LiveViews {
    fn bits(self) -> u8 {
        u8::from(self.stats)
            | u8::from(self.card_signals) << 1
            | u8::from(self.group_quality) << 2
            | u8::from(self.audio_quality) << 3
            | u8::from(self.diagnostics) << 4
    }
}

/// Measurement events stay off the WebView unless a visible view requested them.
fn wanted(event: &ApiEvent, views: u8) -> bool {
    let bit = match event {
        ApiEvent::Stats(_) | ApiEvent::InputStats(_) => 0b0_0101,
        ApiEvent::DeviceCardSignals(_) => 0b0_0010,
        ApiEvent::ConnectionQuality(_) => 0b0_0100,
        ApiEvent::AudioQuality(_) | ApiEvent::AudioStats(_) => 0b0_1000,
        ApiEvent::DiagnosticsStats(_) => 0b1_0000,
        _ => return true,
    };
    views & bit != 0
}

pub struct AndroidHost {
    host: Mutex<Option<AgentHost>>,
    api: Arc<dyn AgentApi>,
    subscription: SubscriptionId,
    shared: Arc<Shared>,
    display: Arc<AndroidDisplay>,
    clipboard: Arc<AndroidClipboard>,
    views: Arc<AtomicU8>,
}

impl std::fmt::Debug for AndroidHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AndroidHost").finish_non_exhaustive()
    }
}

impl AndroidHost {
    pub fn start(config: StartConfig, platform: Arc<dyn Platform>) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        let shared = Arc::new(Shared::default());
        *shared.screen.write().unwrap_or_else(|e| e.into_inner()) = config.screen;
        *shared.platform.write().unwrap_or_else(|e| e.into_inner()) = Some(platform.clone());
        let input = Arc::new(AndroidInput::new(shared.clone()));
        let display = Arc::new(AndroidDisplay::new(shared.clone()));
        let clipboard = Arc::new(AndroidClipboard::new(shared.clone()));
        let net = config.network.unwrap_or_else(|| NetworkConfig {
            data_dir: config.data_dir.clone(),
            device_name: config.device_name.clone(),
            os: "android".into(),
            ..NetworkConfig::default()
        });
        let opts = NodeOptions {
            net,
            input,
            displays: vec![crate::display::display_info(&config.screen)],
            display_backend: Some(display.clone()),
            clipboard: Some(clipboard.clone()),
            mode_switch_in_callback: false,
            capture_reports_key_repeat: false,
            cursor_trail: false,
        };
        let host = AgentHost::start_embedded(opts)?;
        let api = host.api();
        let views = Arc::new(AtomicU8::new(0));
        let filter = views.clone();
        let sink = platform;
        let subscription = api.subscribe(Arc::new(move |event: &ApiEvent| {
            if !wanted(event, filter.load(Ordering::Relaxed)) {
                return;
            }
            match serde_json::to_string(event) {
                Ok(json) => sink.event(&json),
                Err(error) => tracing::warn!(%error, "event not serializable"),
            }
        }));
        tracing::info!("android backend started");
        Ok(Self {
            host: Mutex::new(Some(host)),
            api,
            subscription,
            shared,
            display,
            clipboard,
            views,
        })
    }

    /// One `{"method": ..., "params": ...}` request. Returns
    /// `{"ok": reply}` or `{"err": ApiError}` as JSON. May block on the network:
    /// never call it on the Android main thread.
    pub fn call(&self, request: &str) -> String {
        let result = serde_json::from_str::<Request>(request)
            .map_err(|error| ApiError::InvalidArgument {
                message: error.to_string(),
            })
            .and_then(|request| oink_agent::api::dispatch(&*self.api, request));
        let value = match result {
            Ok(reply) => serde_json::json!({ "ok": reply }),
            Err(error) => serde_json::json!({ "err": error }),
        };
        value.to_string()
    }

    /// The running backend (host tests drive the peer side through it).
    pub fn node(&self) -> Option<oink_agent::node::Node> {
        self.host
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|host| host.node().clone())
    }

    pub fn set_live_views(&self, json: &str) {
        let views = serde_json::from_str::<LiveViews>(json).unwrap_or_default();
        self.views.store(views.bits(), Ordering::Relaxed);
    }

    pub fn set_screen(&self, screen: Screen) {
        let changed = {
            let mut current = self.shared.screen.write().unwrap_or_else(|e| e.into_inner());
            let changed = *current != screen;
            *current = screen;
            changed
        };
        if changed {
            self.display.changed();
        }
    }

    pub fn set_accessibility(&self, connected: bool) {
        self.shared.accessibility.store(connected, Ordering::Release);
    }

    pub fn clipboard_text(&self, text: Option<String>) {
        self.clipboard.local_text(text);
    }

    pub fn stop(&self) {
        let host = self.host.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(host) = host {
            self.api.unsubscribe(self.subscription);
            host.shutdown_and_wait();
            *self.shared.platform.write().unwrap_or_else(|e| e.into_inner()) = None;
            tracing::info!("android backend stopped");
        }
    }
}

impl Drop for AndroidHost {
    fn drop(&mut self) {
        self.stop();
    }
}
