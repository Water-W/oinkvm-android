//! The native side of the bridge: what the Android app must do for the backend.

use std::sync::{Arc, RwLock};

use oink_foundation::input::MouseButton;

/// Pointer buttons as the Android app sees them (stable JNI integers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Button {
    Primary = 0,
    Secondary = 1,
    Middle = 2,
    Back = 3,
    Forward = 4,
}

impl Button {
    pub fn from_mouse(button: MouseButton) -> Option<Self> {
        Some(match button {
            MouseButton::Left => Self::Primary,
            MouseButton::Right => Self::Secondary,
            MouseButton::Middle => Self::Middle,
            MouseButton::Back => Self::Back,
            MouseButton::Forward => Self::Forward,
            _ => return None,
        })
    }
}

/// Calls from the backend into the Android app. Implementations must return
/// quickly: they are called on the backend's ordered injection worker.
///
/// Coordinates are physical screen pixels of the current orientation.
pub trait Platform: Send + Sync {
    /// The remote pointer moved to `(x, y)`.
    fn pointer(&self, x: f64, y: f64);
    /// A pointer button changed at `(x, y)`.
    fn button(&self, button: Button, pressed: bool, x: f64, y: f64);
    /// Scroll at `(x, y)` by `(dx, dy)` screen pixels; positive `dy` reveals content below.
    fn scroll(&self, x: f64, y: f64, dx: f64, dy: f64);
    /// A key changed. `usage` is the HID keyboard usage (page 0x07) or consumer
    /// usage (page 0x0C, `0x0C << 16 | usage`); `text` is the character the US
    /// layout produces for the current modifiers, when there is one.
    fn key(&self, usage: u32, pressed: bool, modifiers: u8, text: Option<&str>);
    /// The remote pointer left this device (hide the on-screen cursor).
    fn pointer_hidden(&self);
    /// Replace the Android clipboard with `text`.
    fn set_clipboard(&self, text: &str);
    /// One serialized `ApiEvent` for the interface.
    fn event(&self, json: &str);
}

/// The current screen in physical pixels and its density.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub width_px: u32,
    pub height_px: u32,
    /// Android `DisplayMetrics.density` (pixels per dp).
    pub density: f64,
    /// Physical pixels per inch, when the device reports plausible values.
    pub xdpi: f64,
    pub ydpi: f64,
    /// Clockwise rotation from the natural orientation: 0 / 90 / 180 / 270.
    pub rotation_deg: u16,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            width_px: 1080,
            height_px: 2400,
            density: 2.625,
            xdpi: 0.0,
            ydpi: 0.0,
            rotation_deg: 0,
        }
    }
}

impl Screen {
    /// Global coordinates are density-independent pixels, like macOS points:
    /// a phone does not become a giant screen in the shared layout.
    pub fn units_w(&self) -> f64 {
        f64::from(self.width_px) / self.density.max(0.5)
    }
    pub fn units_h(&self) -> f64 {
        f64::from(self.height_px) / self.density.max(0.5)
    }
    pub fn to_pixels(&self, x: f64, y: f64) -> (f64, f64) {
        let d = self.density.max(0.5);
        (
            (x * d).clamp(0.0, f64::from(self.width_px.saturating_sub(1))),
            (y * d).clamp(0.0, f64::from(self.height_px.saturating_sub(1))),
        )
    }
}

/// Shared, replaceable state between the JNI layer and the backends.
#[derive(Default)]
pub struct Shared {
    pub screen: RwLock<Screen>,
    /// The accessibility service is connected and can dispatch gestures.
    pub accessibility: std::sync::atomic::AtomicBool,
    pub platform: RwLock<Option<Arc<dyn Platform>>>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("screen", &*self.screen.read().unwrap_or_else(|e| e.into_inner()))
            .finish_non_exhaustive()
    }
}

impl Shared {
    pub fn screen(&self) -> Screen {
        *self.screen.read().unwrap_or_else(|e| e.into_inner())
    }
    pub fn platform(&self) -> Option<Arc<dyn Platform>> {
        self.platform.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn accessibility(&self) -> bool {
        self.accessibility.load(std::sync::atomic::Ordering::Acquire)
    }
}
