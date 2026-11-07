//! Input backend for a controlled Android device.
//!
//! Android exposes no global capture, so the capture loop only runs the
//! router's owner (timers, queue wakeups) and never reports local events.
//! Injected events are translated into [`Platform`] calls in screen pixels.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use oink_foundation::Point;
use oink_foundation::input::{
    CaptureMode, FeelProfile, InjectEvent, InputPermissions, KeyCode, PermissionState, ScrollUnit,
};
use oink_input::capture_loop::CaptureLoop;
use oink_input::{EventHandler, InputBackend, InputError};

use crate::keys::{Modifiers, us_text};
use crate::platform::{Button, Platform, Shared};

/// Density-independent pixels per scrolled line.
const LINE_DP: f64 = 16.0;
const CAPS_LOCK: u16 = 0x39;

#[derive(Debug, Default)]
struct State {
    /// Pointer position in global units (dp).
    cursor: Point,
    modifiers: Modifiers,
    caps: bool,
}

struct OwnerLoop {
    thread: std::thread::Thread,
    stop: Arc<AtomicBool>,
    join: std::thread::JoinHandle<()>,
}

/// The input backend. Cheap to share; all methods may be called from any thread.
pub struct AndroidInput {
    shared: Arc<Shared>,
    state: Mutex<State>,
    owner: Mutex<Option<OwnerLoop>>,
}

impl std::fmt::Debug for AndroidInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AndroidInput").finish_non_exhaustive()
    }
}

impl AndroidInput {
    pub fn new(shared: Arc<Shared>) -> Self {
        let screen = shared.screen();
        Self {
            shared,
            state: Mutex::new(State {
                cursor: Point::new(screen.units_w() / 2.0, screen.units_h() / 2.0),
                ..Default::default()
            }),
            owner: Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn platform(&self) -> Result<Arc<dyn Platform>, InputError> {
        self.shared
            .platform()
            .ok_or_else(|| InputError::Platform("Android platform not attached".into()))
    }

    fn clamp(&self, p: Point) -> Point {
        let screen = self.shared.screen();
        Point::new(
            p.x.clamp(0.0, (screen.units_w() - 1.0).max(0.0)),
            p.y.clamp(0.0, (screen.units_h() - 1.0).max(0.0)),
        )
    }

    fn move_to(&self, platform: &dyn Platform, p: Point) {
        let p = self.clamp(p);
        self.lock().cursor = p;
        let (x, y) = self.shared.screen().to_pixels(p.x, p.y);
        platform.pointer(x, y);
    }

    fn cursor_pixels(&self) -> (f64, f64) {
        let p = self.lock().cursor;
        self.shared.screen().to_pixels(p.x, p.y)
    }

    fn key(&self, platform: &dyn Platform, code: KeyCode, pressed: bool) {
        if code.page() == KeyCode::PAGE_CONSUMER {
            platform.key(code.0, pressed, self.lock().modifiers.0, None);
            return;
        }
        if code.page() != KeyCode::PAGE_KEYBOARD {
            return;
        }
        let usage = code.usage();
        let (modifiers, text) = {
            let mut state = self.lock();
            if state.modifiers.update(usage, pressed) {
                (state.modifiers, None)
            } else {
                if usage == CAPS_LOCK && pressed {
                    state.caps = !state.caps;
                }
                let text = (pressed && !state.modifiers.command())
                    .then(|| us_text(usage, state.modifiers.shift(), state.caps))
                    .flatten();
                (state.modifiers, text)
            }
        };
        let mut buffer = [0u8; 4];
        platform.key(
            u32::from(usage),
            pressed,
            modifiers.0,
            text.map(|c| &*c.encode_utf8(&mut buffer)),
        );
    }
}

impl InputBackend for AndroidInput {
    fn start(&self, _handler: EventHandler) -> Result<(), InputError> {
        // No local capture exists; owners use `start_loop`.
        Ok(())
    }

    fn start_loop(&self, mut owner: Box<dyn CaptureLoop>) -> Result<(), InputError> {
        let mut slot = self.owner.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Err(InputError::Platform("capture already started".into()));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let (ready, started) = std::sync::mpsc::sync_channel(1);
        let join = std::thread::Builder::new()
            .name("oink-android-input".into())
            .spawn(move || {
                let thread = std::thread::current();
                let wake = thread.clone();
                owner.set_wake(Arc::new(move || wake.unpark()));
                let mut ready = Some((ready, thread));
                let mut available = false;
                while !stopping.load(Ordering::Acquire) {
                    let due = owner.poll(Instant::now());
                    if owner.stopped() {
                        break;
                    }
                    // Local "capture" is always empty here; report it attached
                    // so owners publish an ordinary, controllable device.
                    let requested = owner.capture_requested();
                    if requested != available {
                        available = requested;
                        if requested {
                            owner.capture_available(true);
                        } else {
                            owner.capture_detached();
                        }
                    }
                    if let Some((ready, thread)) = ready.take() {
                        let _ = ready.send(thread);
                    }
                    match due {
                        Some(due) => std::thread::park_timeout(due.saturating_duration_since(Instant::now())),
                        None => std::thread::park(),
                    }
                }
            })
            .map_err(|e| InputError::Platform(e.to_string()))?;
        let thread = started
            .recv()
            .map_err(|_| InputError::Platform("input owner stopped during startup".into()))?;
        *slot = Some(OwnerLoop { thread, stop, join });
        Ok(())
    }

    fn capture_active(&self) -> Option<bool> {
        Some(self.owner.lock().unwrap_or_else(|e| e.into_inner()).is_some())
    }

    fn stop(&self) {
        let owner = self.owner.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(owner) = owner {
            owner.stop.store(true, Ordering::Release);
            owner.thread.unpark();
            let _ = owner.join.join();
        }
    }

    fn set_mode(&self, _mode: CaptureMode) -> Result<(), InputError> {
        Ok(())
    }

    fn restore_local(&self, at: Point) -> Result<(), InputError> {
        self.lock().cursor = self.clamp(at);
        Ok(())
    }

    fn stop_injection_session(&self) {
        if let Some(platform) = self.shared.platform() {
            platform.pointer_hidden();
        }
    }

    fn injection_available(&self) -> Option<bool> {
        Some(self.shared.accessibility() && self.shared.platform().is_some())
    }

    fn inject(&self, event: &InjectEvent) -> Result<(), InputError> {
        let platform = self.platform()?;
        match event {
            InjectEvent::MoveTo { pos } | InjectEvent::MoveToWithDelta { pos, .. } => {
                self.move_to(&*platform, *pos);
            }
            InjectEvent::MoveRelative { dx, dy } => {
                let p = self.lock().cursor;
                let d = self.shared.screen().density.max(0.5);
                self.move_to(&*platform, Point::new(p.x + dx / d, p.y + dy / d));
            }
            InjectEvent::Button { button, pressed } => {
                if let Some(button) = Button::from_mouse(*button) {
                    let (x, y) = self.cursor_pixels();
                    platform.button(button, *pressed, x, y);
                }
            }
            InjectEvent::Scroll { dx, dy, unit, .. } => {
                let screen = self.shared.screen();
                let per = match unit {
                    ScrollUnit::Line => LINE_DP * screen.density,
                    _ => screen.density,
                };
                let (x, y) = self.cursor_pixels();
                // Desktop wheels report positive values for "scroll up";
                // the platform contract uses positive for revealing content below.
                platform.scroll(x, y, 0.0 - dx * per, 0.0 - dy * per);
            }
            InjectEvent::TouchpadScroll { wheel, .. } => {
                return self.inject(&InjectEvent::Scroll {
                    dx: wheel.dx,
                    dy: wheel.dy,
                    unit: wheel.unit,
                    phase: wheel.phase,
                });
            }
            InjectEvent::Key { code, pressed } | InjectEvent::KeyWithoutLock { code, pressed } => {
                self.key(&*platform, *code, *pressed);
            }
            InjectEvent::Volume { code, pressed, .. } => {
                self.key(&*platform, *code, *pressed);
            }
            InjectEvent::Gesture(_) | InjectEvent::TouchpadContacts(_) => {}
        }
        Ok(())
    }

    fn cursor_position(&self) -> Result<Point, InputError> {
        Ok(self.lock().cursor)
    }

    fn feel_profile(&self) -> FeelProfile {
        FeelProfile {
            wheel_lines_per_notch: Some(3.0),
            ..Default::default()
        }
    }

    fn permissions(&self) -> InputPermissions {
        InputPermissions {
            accessibility: if self.shared.accessibility() {
                PermissionState::Granted
            } else {
                PermissionState::Denied
            },
            input_monitoring: PermissionState::NotApplicable,
            secure_input_active: false,
        }
    }

    fn display_power_observation(&self) -> Result<Box<dyn oink_input::display_power::Observation>, InputError> {
        Err(InputError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Screen;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);
    impl Platform for Recorder {
        fn pointer(&self, x: f64, y: f64) {
            self.0.lock().unwrap().push(format!("pointer {x:.0} {y:.0}"));
        }
        fn button(&self, button: Button, pressed: bool, x: f64, y: f64) {
            self.0
                .lock()
                .unwrap()
                .push(format!("button {button:?} {pressed} {x:.0} {y:.0}"));
        }
        fn scroll(&self, _x: f64, _y: f64, dx: f64, dy: f64) {
            self.0.lock().unwrap().push(format!("scroll {dx:.0} {dy:.0}"));
        }
        fn key(&self, usage: u32, pressed: bool, modifiers: u8, text: Option<&str>) {
            self.0
                .lock()
                .unwrap()
                .push(format!("key {usage:#x} {pressed} {modifiers} {text:?}"));
        }
        fn pointer_hidden(&self) {}
        fn set_clipboard(&self, _text: &str) {}
        fn event(&self, _json: &str) {}
    }

    fn backend() -> (AndroidInput, Arc<Recorder>) {
        let shared = Arc::new(Shared::default());
        *shared.screen.write().unwrap() = Screen {
            width_px: 1000,
            height_px: 2000,
            density: 2.0,
            ..Default::default()
        };
        let recorder = Arc::new(Recorder::default());
        *shared.platform.write().unwrap() = Some(recorder.clone());
        (AndroidInput::new(shared), recorder)
    }

    #[test]
    fn pointer_and_buttons_use_clamped_screen_pixels() {
        let (input, rec) = backend();
        input
            .inject(&InjectEvent::MoveTo {
                pos: Point::new(100.0, 50.0),
            })
            .unwrap();
        input
            .inject(&InjectEvent::Button {
                button: oink_foundation::input::MouseButton::Left,
                pressed: true,
            })
            .unwrap();
        input
            .inject(&InjectEvent::MoveTo {
                pos: Point::new(900.0, -5.0),
            })
            .unwrap();
        assert_eq!(
            *rec.0.lock().unwrap(),
            ["pointer 200 100", "button Primary true 200 100", "pointer 998 0"]
        );
        assert_eq!(input.cursor_position().unwrap(), Point::new(499.0, 0.0));
    }

    #[test]
    fn wheel_lines_scroll_down_reveals_content_below() {
        let (input, rec) = backend();
        input
            .inject(&InjectEvent::Scroll {
                dx: 0.0,
                dy: -1.0,
                unit: ScrollUnit::Line,
                phase: oink_foundation::input::ScrollPhase::None,
            })
            .unwrap();
        assert_eq!(*rec.0.lock().unwrap(), ["scroll 0 32"]);
    }

    #[test]
    fn keys_carry_us_text_unless_a_command_modifier_is_held() {
        let (input, rec) = backend();
        let key = |usage, pressed| {
            input
                .inject(&InjectEvent::Key {
                    code: KeyCode::keyboard(usage),
                    pressed,
                })
                .unwrap()
        };
        key(0xE1, true);
        key(0x04, true);
        key(0x04, false);
        key(0xE1, false);
        key(0xE0, true);
        key(0x06, true);
        let events = rec.0.lock().unwrap().clone();
        assert_eq!(
            events,
            [
                "key 0xe1 true 2 None",
                "key 0x4 true 2 Some(\"A\")",
                "key 0x4 false 2 None",
                "key 0xe1 false 0 None",
                "key 0xe0 true 1 None",
                "key 0x6 true 1 None",
            ]
        );
    }

    #[test]
    fn permissions_follow_the_accessibility_service() {
        let (input, _) = backend();
        assert_eq!(input.permissions().accessibility, PermissionState::Denied);
        assert_eq!(input.injection_available(), Some(false));
        input.shared.accessibility.store(true, Ordering::Release);
        assert_eq!(input.permissions().accessibility, PermissionState::Granted);
        assert_eq!(input.injection_available(), Some(true));
    }
}
