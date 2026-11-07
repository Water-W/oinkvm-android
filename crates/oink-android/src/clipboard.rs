//! Plain-text clipboard.
//!
//! Android only lets the foreground app (or the default IME) read the clipboard,
//! so the app reports text when it can read it ([`AndroidClipboard::local_text`]).
//! Writing is always allowed: text received from other devices is published
//! through [`crate::platform::Platform::set_clipboard`].

use std::collections::HashMap;
use std::io::{self, Read};
use std::sync::{Arc, Mutex};

use oink_clipboard::content::{self, Provider, Translator};
use oink_clipboard::model::*;
use oink_clipboard::{
    ChangeHandler, ClipData, ClipSnapshot, ClipboardBackend, ClipboardError, NativeSpec, Sensitivity,
};
use oink_foundation::{DeviceId, clipboard::ClipPlatform};

use crate::platform::Shared;

/// Android offers use the open wire platform: there is no native Android
/// reconstruction contract, so every receiver selects the portable text format.
const PLATFORM: ClipPlatform = ClipPlatform::Unknown;
const TEXT_FORMAT: &str = "android:utf8-text";
/// Text larger than this is not shared (matches the interactive budget).
const MAX_TEXT: usize = 8 << 20;

#[derive(Default)]
struct Inner {
    text: Option<String>,
    count: u64,
    handlers: Vec<ChangeHandler>,
}

pub struct AndroidClipboard {
    shared: Arc<Shared>,
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for AndroidClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AndroidClipboard").finish_non_exhaustive()
    }
}

fn error<T>(_: T) -> ClipboardError {
    ClipboardError::Unsupported
}

impl AndroidClipboard {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            inner: Arc::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The app read `text` from the Android clipboard. Unchanged text (including
    /// the echo of a remote write) is not a new copy.
    pub fn local_text(&self, text: Option<String>) {
        let text = text.filter(|t| !t.is_empty() && t.len() <= MAX_TEXT);
        let handlers = {
            let mut inner = self.lock();
            if text.is_none() || inner.text == text {
                return;
            }
            inner.text = text;
            inner.count += 1;
            inner.handlers.clone()
        };
        for handler in handlers {
            handler();
        }
    }

    pub fn text(&self) -> Option<String> {
        self.lock().text.clone()
    }
}

struct TextProvider {
    inventory: Inventory,
    bytes: HashMap<RepresentationId, Vec<u8>>,
    inner: Arc<Mutex<Inner>>,
    count: u64,
}

fn current(inner: &Mutex<Inner>, count: u64) -> io::Result<()> {
    if inner.lock().unwrap_or_else(|e| e.into_inner()).count == count {
        Ok(())
    } else {
        Err(content::unavailable(Unavailable::Superseded))
    }
}

struct Portable;
impl Translator for Portable {
    fn standard(&self, rep: &Representation, bytes: &[u8]) -> io::Result<Vec<ClipData>> {
        match rep.standard {
            Some(StandardFormat::Text) => Ok(vec![ClipData::Text(
                String::from_utf8(bytes.to_vec()).map_err(io::Error::other)?,
            )]),
            _ => Ok(Vec::new()),
        }
    }
}

impl Provider for TextProvider {
    fn inventory(&self) -> Inventory {
        self.inventory.clone()
    }
    fn open(&self, id: RepresentationId) -> io::Result<oink_clipboard::FileReader> {
        current(&self.inner, self.count)?;
        let bytes = self.bytes.get(&id).ok_or(io::ErrorKind::NotFound)?.clone();
        Ok(Box::new(io::Cursor::new(bytes)))
    }
    fn reader_guard(&self, _: RepresentationId) -> io::Result<content::ReaderGuard> {
        let inner = self.inner.clone();
        let count = self.count;
        Ok(Arc::new(move || current(&inner, count)))
    }
    fn translator(&self) -> Arc<dyn Translator> {
        Arc::new(Portable)
    }
}

impl ClipboardBackend for AndroidClipboard {
    fn change_count(&self) -> u64 {
        self.lock().count
    }

    fn read(&self) -> Result<ClipSnapshot, ClipboardError> {
        let inner = self.lock();
        Ok(ClipSnapshot {
            items: inner.text.clone().map(ClipData::Text).into_iter().collect(),
            sensitivity: Sensitivity::Normal,
            change_count: inner.count,
        })
    }

    fn capture(&self, _data: &std::path::Path) -> Result<oink_clipboard::CapturedClipboard, ClipboardError> {
        let snapshot = self.read()?;
        let count = snapshot.change_count;
        let mut inventory = Inventory {
            id: OfferId {
                clip: ClipId {
                    origin: DeviceId([0; 32]),
                    seq: count,
                },
                generation: count,
            },
            platform: PLATFORM,
            items: vec![],
            roots: vec![],
        };
        let mut bytes = HashMap::new();
        if let Some(ClipData::Text(text)) = snapshot.items.first() {
            let id = RepresentationId(1);
            let payload = text.as_bytes().to_vec();
            inventory.items.push(ContentItem {
                virtual_files: false,
                id: ItemId(1),
                file: None,
                representations: vec![Representation {
                    id,
                    format: NativeFormat::Unknown(TEXT_FORMAT.into()),
                    medium: Medium::Bytes,
                    aspect: 1,
                    index: -1,
                    standard: Some(StandardFormat::Text),
                    data: RepresentationData::Reader {
                        size: ContentSize::Known(payload.len() as u64),
                    },
                }],
            });
            bytes.insert(id, payload);
        }
        inventory.validate().map_err(error)?;
        let provider = Arc::new(TextProvider {
            inventory: inventory.clone(),
            bytes,
            inner: self.inner.clone(),
            count,
        });
        Ok(oink_clipboard::CapturedClipboard {
            snapshot: ClipSnapshot {
                items: content::items(&inventory),
                ..snapshot
            },
            content: Some(provider),
        })
    }

    fn write_native(&self, spec: NativeSpec) -> Result<u64, ClipboardError> {
        self.write_native_guarded(spec, Arc::new(|| Ok(())))
    }

    fn write_native_guarded(&self, spec: NativeSpec, guard: content::ReaderGuard) -> Result<u64, ClipboardError> {
        let text_rep = spec
            .inventory
            .items
            .iter()
            .flat_map(|item| &item.representations)
            .find(|rep| {
                rep.standard == Some(StandardFormat::Text)
                    && !matches!(
                        rep.data,
                        RepresentationData::File(_)
                            | RepresentationData::Promise(..)
                            | RepresentationData::Unavailable(_)
                    )
            });
        let Some(rep) = text_rep else {
            // Images, files and rich-only content have no Android target yet.
            return Err(ClipboardError::Unsupported);
        };
        let mut bytes = Vec::new();
        match &rep.data {
            RepresentationData::Inline(value) => bytes.extend_from_slice(value),
            _ => {
                spec.contents
                    .open(rep.id)
                    .map_err(error)?
                    .take(MAX_TEXT as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(error)?;
            }
        }
        if bytes.len() > MAX_TEXT {
            return Err(ClipboardError::Unsupported);
        }
        let text = spec
            .contents
            .translator()
            .standard(rep, &bytes)
            .ok()
            .and_then(|items| {
                items.into_iter().find_map(|item| match item {
                    ClipData::Text(text) => Some(text),
                    _ => None,
                })
            })
            .or_else(|| String::from_utf8(bytes).ok())
            .ok_or(ClipboardError::Unsupported)?;
        guard().map_err(|_| ClipboardError::Busy)?;
        self.write(&[ClipData::Text(text)], spec.sensitivity)
    }

    fn write(&self, items: &[ClipData], _sensitivity: Sensitivity) -> Result<u64, ClipboardError> {
        let text = items
            .iter()
            .find_map(|item| match item {
                ClipData::Text(text) => Some(text.clone()),
                _ => None,
            })
            .ok_or(ClipboardError::Unsupported)?;
        let platform = self.shared.platform().ok_or(ClipboardError::Unsupported)?;
        let count = {
            let mut inner = self.lock();
            inner.text = Some(text.clone());
            inner.count += 1;
            inner.count
        };
        platform.set_clipboard(&text);
        Ok(count)
    }

    fn watch(&self, on_change: ChangeHandler) -> Result<(), ClipboardError> {
        self.lock().handlers.push(on_change);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Button, Platform};

    #[derive(Default)]
    struct Board(Mutex<Vec<String>>);
    impl Platform for Board {
        fn pointer(&self, _: f64, _: f64) {}
        fn button(&self, _: Button, _: bool, _: f64, _: f64) {}
        fn scroll(&self, _: f64, _: f64, _: f64, _: f64) {}
        fn key(&self, _: u32, _: bool, _: u8, _: Option<&str>) {}
        fn pointer_hidden(&self) {}
        fn set_clipboard(&self, text: &str) {
            self.0.lock().unwrap().push(text.into());
        }
        fn event(&self, _: &str) {}
    }

    fn clipboard() -> (AndroidClipboard, Arc<Board>) {
        let shared = Arc::new(Shared::default());
        let board = Arc::new(Board::default());
        *shared.platform.write().unwrap() = Some(board.clone());
        (AndroidClipboard::new(shared), board)
    }

    #[test]
    fn local_copy_is_captured_as_standard_text() {
        let (clip, _) = clipboard();
        let fired = Arc::new(Mutex::new(0));
        let counter = fired.clone();
        clip.watch(Arc::new(move || *counter.lock().unwrap() += 1)).unwrap();
        clip.local_text(Some("hello".into()));
        clip.local_text(Some("hello".into()));
        assert_eq!(*fired.lock().unwrap(), 1);
        let captured = clip.capture(std::path::Path::new("/nonexistent")).unwrap();
        let provider = captured.content.unwrap();
        let inventory = provider.inventory();
        let rep = &inventory.items[0].representations[0];
        assert_eq!(rep.standard, Some(StandardFormat::Text));
        let mut text = String::new();
        provider.open(rep.id).unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "hello");
        clip.local_text(Some("newer".into()));
        assert!(
            provider.open(rep.id).is_err(),
            "a superseded offer must not be readable"
        );
    }

    #[test]
    fn remote_text_is_published_and_its_echo_ignored() {
        let (source, _) = clipboard();
        source.local_text(Some("from windows".into()));
        let captured = source.capture(std::path::Path::new("/nonexistent")).unwrap();
        let provider = captured.content.unwrap();
        let (target, board) = clipboard();
        let fired = Arc::new(Mutex::new(0));
        let counter = fired.clone();
        target.watch(Arc::new(move || *counter.lock().unwrap() += 1)).unwrap();
        let count = target
            .write_native(NativeSpec {
                inventory: provider.inventory(),
                contents: provider,
                files: vec![],
                file_contents: None,
                sensitivity: Sensitivity::Normal,
            })
            .unwrap();
        assert_eq!(count, target.change_count());
        assert_eq!(*board.0.lock().unwrap(), ["from windows"]);
        target.local_text(Some("from windows".into()));
        assert_eq!(*fired.lock().unwrap(), 0);
    }
}
