//! JNI entry points for `com.hachiwii.oink.android.Native` (see `apps/android`).
//!
//! Every native method is synchronous. `nativeStart` and `nativeCall` can block
//! on the network and must run off the Android main thread. Callbacks arrive
//! on backend threads, attached to the JVM for their lifetime.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jfloat, jint, jstring};
use jni::{JNIEnv, JavaVM};

use crate::host::{AndroidHost, StartConfig};
use crate::platform::{Button, Platform, Screen};

static HOST: Mutex<Option<Arc<AndroidHost>>> = Mutex::new(None);

fn host() -> Option<Arc<AndroidHost>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Calls the app's `Native.Callbacks` object.
struct JniPlatform {
    vm: JavaVM,
    callbacks: GlobalRef,
}

impl JniPlatform {
    fn call(&self, name: &str, sig: &str, args: &[JValue<'_, '_>]) {
        let Ok(mut env) = self.vm.attach_current_thread_permanently() else {
            return;
        };
        if env.call_method(self.callbacks.as_obj(), name, sig, args).is_err() {
            // Never unwind a Java exception into the backend.
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
    }

    fn call_text(&self, name: &str, text: &str) {
        let Ok(mut env) = self.vm.attach_current_thread_permanently() else {
            return;
        };
        let Ok(value) = env.new_string(text) else {
            let _ = env.exception_clear();
            return;
        };
        let value = JObject::from(value);
        if env
            .call_method(
                self.callbacks.as_obj(),
                name,
                "(Ljava/lang/String;)V",
                &[JValue::Object(&value)],
            )
            .is_err()
        {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        let _ = env.delete_local_ref(value);
    }
}

impl Platform for JniPlatform {
    fn pointer(&self, x: f64, y: f64) {
        self.call("pointer", "(FF)V", &[JValue::Float(x as f32), JValue::Float(y as f32)]);
    }
    fn button(&self, button: Button, pressed: bool, x: f64, y: f64) {
        self.call(
            "button",
            "(IZFF)V",
            &[
                JValue::Int(button as i32),
                JValue::Bool(u8::from(pressed)),
                JValue::Float(x as f32),
                JValue::Float(y as f32),
            ],
        );
    }
    fn scroll(&self, x: f64, y: f64, dx: f64, dy: f64) {
        self.call(
            "scroll",
            "(FFFF)V",
            &[
                JValue::Float(x as f32),
                JValue::Float(y as f32),
                JValue::Float(dx as f32),
                JValue::Float(dy as f32),
            ],
        );
    }
    fn key(&self, usage: u32, pressed: bool, modifiers: u8, text: Option<&str>) {
        let Ok(mut env) = self.vm.attach_current_thread_permanently() else {
            return;
        };
        let text = match text {
            Some(text) => match env.new_string(text) {
                Ok(value) => JObject::from(value),
                Err(_) => {
                    let _ = env.exception_clear();
                    return;
                }
            },
            None => JObject::null(),
        };
        if env
            .call_method(
                self.callbacks.as_obj(),
                "key",
                "(IZILjava/lang/String;)V",
                &[
                    JValue::Int(usage as i32),
                    JValue::Bool(u8::from(pressed)),
                    JValue::Int(i32::from(modifiers)),
                    JValue::Object(&text),
                ],
            )
            .is_err()
        {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        if !text.is_null() {
            let _ = env.delete_local_ref(text);
        }
    }
    fn pointer_hidden(&self) {
        self.call("pointerHidden", "()V", &[]);
    }
    fn set_clipboard(&self, text: &str) {
        self.call_text("setClipboard", text);
    }
    fn event(&self, json: &str) {
        self.call_text("event", json);
    }
}

fn string(env: &mut JNIEnv<'_>, value: &JString<'_>) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

fn java_string(env: &mut JNIEnv<'_>, value: &str) -> jstring {
    env.new_string(value).map_or(std::ptr::null_mut(), JString::into_raw)
}

#[allow(clippy::too_many_arguments)]
fn screen(width: jint, height: jint, density: jfloat, xdpi: jfloat, ydpi: jfloat, rotation: jint) -> Screen {
    Screen {
        width_px: width.max(1) as u32,
        height_px: height.max(1) as u32,
        density: f64::from(density).max(0.5),
        xdpi: f64::from(xdpi),
        ydpi: f64::from(ydpi),
        rotation_deg: (rotation.rem_euclid(4) * 90) as u16,
    }
}

/// Starts the backend once per process. Returns null on success, or an error message.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_hachiwii_oink_android_Native_nativeStart<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    data_dir: JString<'local>,
    device_name: JString<'local>,
    language: JString<'local>,
    width: jint,
    height: jint,
    density: jfloat,
    xdpi: jfloat,
    ydpi: jfloat,
    rotation: jint,
    callbacks: JObject<'local>,
) -> jstring {
    let mut slot = HOST.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return std::ptr::null_mut();
    }
    let (Some(data_dir), Some(device_name)) = (string(&mut env, &data_dir), string(&mut env, &device_name)) else {
        return java_string(&mut env, "missing data directory or device name");
    };
    let language = string(&mut env, &language).unwrap_or_default();
    let (Ok(vm), Ok(callbacks)) = (env.get_java_vm(), env.new_global_ref(callbacks)) else {
        return java_string(&mut env, "JNI references unavailable");
    };
    let root = PathBuf::from(data_dir);
    let screen = screen(width, height, density, xdpi, ydpi, rotation);
    // A panic must never unwind into the JVM (that aborts the app): report it.
    let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: first call in this process; the library has no threads yet.
        unsafe { crate::host::prepare_process(&root, &language) }
            .map_err(|error| anyhow::anyhow!("prepare app storage: {error}"))?;
        AndroidHost::start(
            StartConfig {
                data_dir: root.clone(),
                device_name,
                screen,
                network: None,
            },
            Arc::new(JniPlatform { vm, callbacks }),
        )
    }));
    match started {
        Ok(Ok(host)) => {
            *slot = Some(Arc::new(host));
            std::ptr::null_mut()
        }
        Ok(Err(error)) => {
            tracing::error!(error = format!("{error:#}"), "android backend start failed");
            java_string(&mut env, &format!("{error:#}"))
        }
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            java_string(&mut env, &format!("backend start panicked: {message}"))
        }
    }
}

/// One JSON request; returns `{"ok": ...}` or `{"err": ...}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hachiwii_oink_android_Native_nativeCall<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    request: JString<'local>,
) -> jstring {
    let reply = match (host(), string(&mut env, &request)) {
        (Some(host), Some(request)) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| host.call(&request)))
            .unwrap_or_else(|_| r#"{"err":{"code":"internal","message":"backend request panicked"}}"#.to_string()),
        (None, _) => r#"{"err":{"code":"unavailable","message":"backend not started"}}"#.to_string(),
        (_, None) => r#"{"err":{"code":"invalid_argument","message":"missing request"}}"#.to_string(),
    };
    java_string(&mut env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hachiwii_oink_android_Native_nativeSetLiveViews<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    views: JString<'local>,
) {
    if let (Some(host), Some(views)) = (host(), string(&mut env, &views)) {
        host.set_live_views(&views);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hachiwii_oink_android_Native_nativeSetScreen<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    width: jint,
    height: jint,
    density: jfloat,
    xdpi: jfloat,
    ydpi: jfloat,
    rotation: jint,
) {
    if let Some(host) = host() {
        host.set_screen(screen(width, height, density, xdpi, ydpi, rotation));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hachiwii_oink_android_Native_nativeSetAccessibility<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    connected: jboolean,
) {
    if let Some(host) = host() {
        host.set_accessibility(connected != 0);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hachiwii_oink_android_Native_nativeClipboardText<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    text: JString<'local>,
) {
    if let Some(host) = host() {
        host.clipboard_text(string(&mut env, &text));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hachiwii_oink_android_Native_nativeStop<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(host) = host {
        host.stop();
    }
}
