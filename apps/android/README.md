# Oink for Android

An Android device joins an Oink group as a **controlled peer**: the keyboard
and mouse of any paired Mac or Windows computer can move onto the phone or
tablet screen, and plain-text clipboard content is shared in both directions.

| Part | Location |
|---|---|
| Backend in-process, Android input/screen/clipboard backends, JNI | `../../crates/oink-android/` |
| Foreground service, accessibility service, WebView host | `app/src/main/java/com/hachiwii/oink/android/` |
| Tauri IPC shim for the shared interface | `web/oink-android.js` |
| Shared interface (pairing, layout, settings) | `../oink/` |

## How it works

- `OinkService` (foreground service) starts the regular backend
  (`oink-agent`) inside the app process through `liboink_android.so`. Discovery
  (mDNS + UDP beacon, with a Wi-Fi multicast lock), pairing, encryption, the
  layout and routing are the same code as on the desktop. The device reports
  `os = "android"` and one display sized in density-independent pixels, with
  its physical size, so it appears at its real size in the shared layout.
- `MainActivity` shows the same React interface as the desktop app in a
  WebView. `oink-android.js` provides the `__TAURI_INTERNALS__` surface the
  interface uses: `api_call` goes to the backend over JNI and backend events
  arrive as `agent-event`. On narrow screens the viewport is widened to the
  interface's minimum width and scaled to fit.
- Android cannot inject raw input, so `InputService` (an
  AccessibilityService the user enables once) translates remote input:
  - an overlay arrow follows the pointer;
  - left click = tap, held click = long press, drag = swipe along the path
    (replayed on release);
  - the wheel or touchpad scroll = a short swipe under the pointer;
  - right click / Esc / mouse back = Back, middle click = Home, forward = Recents;
  - printable keys edit the focused text field (US layout);
    Backspace, Delete, Enter, ←/→, Home/End, Tab and Ctrl/⌘ + A/C/X/V use
    accessibility node actions; volume keys adjust the media volume.
- Clipboard: text from other devices is written to the Android clipboard
  immediately. Android 10+ only lets the focused app read the clipboard, so
  text copied on the phone is sent when Oink comes to the foreground, from the
  notification's **Send clipboard** action, or via **Share → Send to Oink
  clipboard**.

Not supported yet: controlling other devices *from* Android, files, images
and rich text on the clipboard, drag and drop, audio, IME composition from
the source computer, and password fields (Android hides their text from
accessibility services).

## Build

Requirements: Rust (see `rust-toolchain.toml`) with
`rustup target add aarch64-linux-android` (and `x86_64-linux-android` for
emulators), Android SDK platform 35 + build-tools, NDK `27.2.12479018`
(`ANDROID_HOME` set), JDK 17+, Gradle 8.13+, Node/npm for the interface.

```bash
npm ci
python apps/android/build.py                               # debug-signed APK for arm64-v8a
python apps/android/build.py --abi arm64-v8a --abi x86_64  # also for x86_64 emulators
adb install -r apps/android/app/build/outputs/apk/debug/app-debug.apk
```

## Try it

1. Install and open Oink on the Android device (same network as the computer).
2. Tap **Open settings** in the banner and turn on **Oink remote control**
   under Accessibility (installed apps).
3. Pair from either side as with any other device (nearby list, or the
   phone's address from its device page).
4. In the arrangement, place the phone screen next to the computer's screen,
   then move the computer's pointer across that edge.

If Android says the setting is restricted ("Restricted setting" / "受限制的设置"),
which Android 13+ does for apps installed from a downloaded file: open
**Settings → Apps → Oink**, tap **⋮** in the top-right corner, choose **Allow
restricted settings**, confirm, and turn the accessibility service on again.
Apps installed with `adb install` are not restricted.

Logs and diagnostics are written under the app's private storage and can be
exported from the interface like on the desktop.

## Development

- `cargo test -p oink-android` runs the backends' unit tests and an end-to-end
  test: a desktop backend with synthetic OS input pairs with the Android host
  through the JSON API, moves its pointer onto the phone, clicks, types and
  syncs clipboard text both ways.
- `cargo run -p oink-android --example preview -- --paired` serves the APK's
  web assets (`python apps/android/build.py --skip-rust --skip-apk` first)
  against a real Android host paired with a desktop backend, at
  <http://127.0.0.1:1450/index.html?view=main>, for working on the interface
  in a browser with phone emulation.
