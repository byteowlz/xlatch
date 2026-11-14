//! App-owned macOS window controls through the audited `xlatch-window-controls`
//! adapter, the workspace's only unsafe boundary. This module stays safe: it only
//! keeps the installed controls on the main thread and recolours them.
use std::cell::RefCell;

use anyhow::{Context as _, Result};
use rune::{Color, WindowHandle};
use xlatch_window_controls::{Layout, WindowControls};

thread_local! {
    /// The controls on the current main window. Main-thread only, like AppKit itself.
    static CONTROLS: RefCell<Option<WindowControls>> = const { RefCell::new(None) };
}

/// The control group's placement in the app's titlebar band.
pub fn layout() -> Layout {
    Layout::centered(f64::from(crate::ui::TITLEBAR_HEIGHT))
}

fn encoded_rgba(color: Color) -> Result<u32> {
    // Rune emits #rrggbbaa; the application palette parser accepts #rrggbb.
    let encoded = color.hex();
    Ok(u32::from_str_radix(
        encoded.strip_prefix('#').context("missing RGBA prefix")?,
        16,
    )?)
}

/// Installs the controls on `handle`'s window, or recolours them if already there.
pub fn apply(handle: &WindowHandle, foreground: Color) -> Result<()> {
    let number = isize::try_from(
        handle
            .window_number()
            .context("native window is not ready")?,
    )?;
    let rgba = encoded_rgba(foreground)?;
    CONTROLS.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(controls) = slot.as_ref().filter(|c| c.window_number() == number) {
            controls.set_tint(rgba);
            return Ok(());
        }
        // Restore any previous window's stock controls before installing anew.
        *slot = None;
        *slot = Some(WindowControls::install(number, layout(), rgba)?);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_tint_decodes_rune_rgba_instead_of_palette_rgb() -> Result<()> {
        let color = crate::appearance::color(0x00df_e8e1);
        anyhow::ensure!(encoded_rgba(color)? == 0xdfe8_e1ff);
        anyhow::ensure!(encoded_rgba(color.alpha(0.5))? == 0xdfe8_e180);
        Ok(())
    }
}
