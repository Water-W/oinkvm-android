//! Android host for the Oink backend.
//!
//! The Android app (`apps/android`) loads this library, starts one in-process
//! backend [`host::AndroidHost`] from a foreground service and talks to it
//! through the same JSON `Request` / `Reply` / `ApiEvent` contract the desktop
//! interface uses over local IPC.
//!
//! Android cannot capture global keyboard and mouse input, so this device is a
//! *controlled* peer: other computers move their pointer onto the phone or
//! tablet screen, and the platform layer turns the injected events into
//! accessibility gestures, text edits and global actions ([`platform::Platform`]).
//! The clipboard carries plain text in both directions.
//!
//! Everything except the JNI glue is platform independent so it can be
//! exercised by host tests.

pub mod clipboard;
pub mod display;
pub mod host;
pub mod input;
pub mod keys;
pub mod platform;

#[cfg(target_os = "android")]
mod jni_bridge;
