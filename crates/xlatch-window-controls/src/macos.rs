//! AppKit adapter for the app-owned window controls.
//!
//! Invariants every caller relies on, enforced here rather than by callers:
//! - All AppKit work happens on the main thread ([`MainThreadMarker`]); the
//!   handle holds main-thread-only objects and is therefore neither `Send` nor `Sync`.
//! - The window is held weakly. Each control targets its stock counterpart
//!   (`performClick:`), and AppKit keeps control targets weak, so a closed window
//!   is never kept alive or messaged through a dangling target.
//! - Exactly one control group is visible and accessible: installing hides the
//!   stock buttons and points the window's accessibility relationships at the new
//!   controls; dropping removes them, unhides the stock buttons and restores the
//!   relationships.
#![expect(
    unsafe_code,
    reason = "objc2 cannot type-check AppKit control targets/actions, the superview lookup, \
              accessibility constants and the window's accessibility button relationships"
)]

use std::fmt;

use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityCloseButtonSubrole, NSAccessibilityFullScreenButtonSubrole,
    NSAccessibilityMinimizeButtonSubrole, NSAccessibilitySubrole, NSApplication,
    NSAutoresizingMaskOptions, NSBezelStyle, NSButton, NSCellImagePosition, NSColor, NSImage,
    NSImageSymbolConfiguration, NSWindow, NSWindowButton, NSWindowOrderingMode,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::{CONTROLS, Control, Layout};

/// Why the controls could not be installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// AppKit objects may only be touched on the main thread.
    NotMainThread,
    /// No window of this application has the given window number.
    WindowMissing,
    /// The window has no frame view to host the controls.
    FrameMissing,
    /// The window has no stock button to forward this control to.
    StockMissing(Control),
    /// The system has no symbol image for this control.
    SymbolMissing(Control),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotMainThread => f.write_str("window controls require the main thread"),
            Self::WindowMissing => f.write_str("the native window no longer exists"),
            Self::FrameMissing => f.write_str("the native window has no frame view"),
            Self::StockMissing(control) => {
                write!(f, "the window has no native {} button", control.label())
            }
            Self::SymbolMissing(control) => {
                write!(f, "no system symbol for the {} control", control.label())
            }
        }
    }
}

impl std::error::Error for Error {}

/// The app-owned controls installed on one window. Dropping restores the stock ones.
#[derive(Debug)]
pub struct WindowControls {
    window: Weak<NSWindow>,
    number: isize,
    buttons: [Retained<NSButton>; 3],
}

impl WindowControls {
    /// Installs the controls on this application's window numbered `number`,
    /// tinted `rgba` (`0xRRGGBBAA`, sRGB).
    ///
    /// # Errors
    ///
    /// Fails off the main thread, when the window or its frame view is gone, or
    /// when a stock button or system symbol is unavailable. Nothing is changed then.
    pub fn install(number: isize, layout: Layout, rgba: u32) -> Result<Self, Error> {
        let mtm = MainThreadMarker::new().ok_or(Error::NotMainThread)?;
        let window = NSApplication::sharedApplication(mtm)
            .windows()
            .iter()
            .find(|window| window.windowNumber() == number)
            .ok_or(Error::WindowMissing)?;
        let content = window.contentView().ok_or(Error::FrameMissing)?;
        // SAFETY: `content` is the live content view of a window on the main thread.
        // `superview` returns the retained frame view (or nil), which AppKit owns.
        let frame = unsafe { content.superview() }.ok_or(Error::FrameMissing)?;
        let stock = stock_buttons(&window);
        let tint = color(rgba);
        let flipped = frame.isFlipped();
        let rects = layout.frames(frame.bounds().size.height, flipped);
        let [close, minimize, full_screen] =
            [0, 1, 2].map(|i| control(mtm, CONTROLS[i], stock[i].as_deref(), &tint, layout.size));
        let buttons = [close?, minimize?, full_screen?];
        // Pinned to the top-leading corner as the window resizes.
        let mask = if flipped {
            NSAutoresizingMaskOptions::ViewMaxXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin
        } else {
            NSAutoresizingMaskOptions::ViewMaxXMargin | NSAutoresizingMaskOptions::ViewMinYMargin
        };
        for (button, [x, y, width, height]) in buttons.iter().zip(rects) {
            button.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
            button.setAutoresizingMask(mask);
            frame.addSubview_positioned_relativeTo(button, NSWindowOrderingMode::Above, None);
        }
        for button in stock.iter().flatten() {
            button.setHidden(true);
        }
        relate(&window, [&buttons[0], &buttons[1], &buttons[2]]);
        Ok(Self {
            window: Weak::from_retained(&window),
            number,
            buttons,
        })
    }

    /// The window number these controls were installed on.
    #[must_use]
    pub const fn window_number(&self) -> isize {
        self.number
    }

    /// Recolours the control glyphs to `rgba` (`0xRRGGBBAA`, sRGB).
    pub fn set_tint(&self, rgba: u32) {
        let tint = color(rgba);
        for button in &self.buttons {
            button.setContentTintColor(Some(&tint));
        }
    }
}

impl Drop for WindowControls {
    fn drop(&mut self) {
        for button in &self.buttons {
            button.removeFromSuperview();
        }
        let Some(window) = self.window.load() else {
            return;
        };
        let stock = stock_buttons(&window);
        if let [Some(close), Some(minimize), Some(zoom)] = &stock {
            relate(&window, [close, minimize, zoom]);
        }
        for button in stock.iter().flatten() {
            button.setHidden(false);
        }
    }
}

fn stock_buttons(window: &NSWindow) -> [Option<Retained<NSButton>>; 3] {
    [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .map(|kind| window.standardWindowButton(kind))
}

fn color(rgba: u32) -> Retained<NSColor> {
    let channel = |shift: u32| f64::from((rgba >> shift) & 0xff) / 255.;
    NSColor::colorWithSRGBRed_green_blue_alpha(channel(24), channel(16), channel(8), channel(0))
}

fn subrole(control: Control) -> &'static NSAccessibilitySubrole {
    match control {
        // SAFETY: AppKit's immutable, process-lifetime accessibility constants.
        Control::Close => unsafe { NSAccessibilityCloseButtonSubrole },
        // SAFETY: as above.
        Control::Minimize => unsafe { NSAccessibilityMinimizeButtonSubrole },
        // SAFETY: as above.
        Control::FullScreen => unsafe { NSAccessibilityFullScreenButtonSubrole },
    }
}

fn control(
    mtm: MainThreadMarker,
    control: Control,
    stock: Option<&NSButton>,
    tint: &NSColor,
    size: f64,
) -> Result<Retained<NSButton>, Error> {
    let stock = stock.ok_or(Error::StockMissing(control))?;
    let label = NSString::from_str(control.label());
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(size * 0.5, 0.);
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(control.symbol()),
        Some(&label),
    )
    .and_then(|image| image.imageWithSymbolConfiguration(&config))
    .ok_or(Error::SymbolMissing(control))?;
    let button = NSButton::new(mtm);
    button.setBezelStyle(NSBezelStyle::AccessoryBarAction);
    button.setShowsBorderOnlyWhileMouseInside(true);
    button.setTitle(&NSString::new());
    button.setImage(Some(&image));
    button.setImagePosition(NSCellImagePosition::ImageOnly);
    button.setContentTintColor(Some(tint));
    button.setRefusesFirstResponder(true);
    button.setToolTip(Some(&label));
    button.setAccessibilityLabel(Some(&label));
    button.setAccessibilitySubrole(Some(subrole(control)));
    button.setEnabled(stock.isEnabled());
    let target: &AnyObject = stock;
    // SAFETY: AppKit holds control targets weakly. The stock button belongs to the
    // same window's frame and outlives this control, which `Drop` removes; a
    // closed window releases both together.
    unsafe { button.setTarget(Some(target)) };
    // SAFETY: `performClick:` is NSButton's own one-argument action: the stock
    // button runs its native close, minimize or full-screen behaviour.
    unsafe { button.setAction(Some(sel!(performClick:))) };
    Ok(button)
}

/// Points the window's accessibility close, minimize, zoom and full-screen
/// relationships at `[close, minimize, full_screen]`.
fn relate(window: &NSWindow, [close, minimize, full_screen]: [&NSButton; 3]) {
    let [close, minimize, full_screen]: [&AnyObject; 3] = [close, minimize, full_screen];
    // SAFETY: each relationship names a button in this window's own frame view,
    // on the main thread, as AppKit's accessibility API expects.
    unsafe { window.setAccessibilityCloseButton(Some(close)) };
    // SAFETY: as above.
    unsafe { window.setAccessibilityMinimizeButton(Some(minimize)) };
    // SAFETY: as above.
    unsafe { window.setAccessibilityZoomButton(Some(full_screen)) };
    // SAFETY: as above.
    unsafe { window.setAccessibilityFullScreenButton(Some(full_screen)) };
}
