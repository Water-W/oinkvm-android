//! The device screen as one display in density-independent pixels.

use std::sync::{Arc, Mutex};

use oink_display::{ChangeHandler, DisplayBackend, DisplayError, DisplayInfo};
use oink_foundation::display::REFERENCE_DIAGONAL_METERS;
use oink_foundation::{Rect, Size};

use crate::platform::{Screen, Shared};

const METERS_PER_INCH: f64 = 0.0254;

pub struct AndroidDisplay {
    shared: Arc<Shared>,
    handlers: Mutex<Vec<ChangeHandler>>,
}

impl std::fmt::Debug for AndroidDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AndroidDisplay").finish_non_exhaustive()
    }
}

impl AndroidDisplay {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            handlers: Mutex::default(),
        }
    }

    /// Notify the backend after [`Shared::screen`] changed (rotation, resize).
    pub fn changed(&self) {
        let handlers = self.handlers.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for handler in handlers {
            handler();
        }
    }
}

/// Plausible physical size from the reported pixel density.
fn reference_extent(screen: &Screen) -> Option<Size> {
    let plausible = |dpi: f64| (72.0..=1200.0).contains(&dpi);
    if !plausible(screen.xdpi) || !plausible(screen.ydpi) {
        return None;
    }
    let w = f64::from(screen.width_px) / screen.xdpi * METERS_PER_INCH;
    let h = f64::from(screen.height_px) / screen.ydpi * METERS_PER_INCH;
    Some(Size::new(w / REFERENCE_DIAGONAL_METERS, h / REFERENCE_DIAGONAL_METERS))
}

pub fn display_info(screen: &Screen) -> DisplayInfo {
    DisplayInfo {
        key: "android:builtin".into(),
        name: "Built-in Screen".into(),
        bounds: Rect::new(0.0, 0.0, screen.units_w(), screen.units_h()),
        pixel_width: screen.width_px,
        pixel_height: screen.height_px,
        reference_extent: reference_extent(screen),
        rotation_deg: screen.rotation_deg,
        is_primary: true,
        is_builtin: true,
        is_virtual: false,
    }
}

impl DisplayBackend for AndroidDisplay {
    fn displays(&self) -> Result<Vec<DisplayInfo>, DisplayError> {
        Ok(vec![display_info(&self.shared.screen())])
    }

    fn watch(&self, on_change: ChangeHandler) -> Result<(), DisplayError> {
        self.handlers.lock().unwrap_or_else(|e| e.into_inner()).push(on_change);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_is_reported_in_dp_with_physical_extent() {
        let screen = Screen {
            width_px: 1080,
            height_px: 2400,
            density: 2.7,
            xdpi: 400.0,
            ydpi: 400.0,
            rotation_deg: 0,
        };
        let info = display_info(&screen);
        // 2400 / 2.7 = 888.9 dp: the shared layout needs whole units.
        assert_eq!(info.bounds, Rect::new(0.0, 0.0, 400.0, 889.0));
        assert_eq!(info.pixel_width, 1080);
        let extent = info.reference_extent.unwrap();
        assert!((extent.w - 1080.0 / 400.0 * 0.0254 / REFERENCE_DIAGONAL_METERS).abs() < 1e-9);
        assert!(display_info(&Screen { xdpi: 0.0, ..screen }).reference_extent.is_none());
    }
}
