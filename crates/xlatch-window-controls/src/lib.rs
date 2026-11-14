//! App-owned window controls for the xlatch desktop app.
//!
//! The geometry here is platform-neutral and shared with the offscreen preview.
//! On macOS, [`WindowControls`] replaces the stock traffic lights with real
//! `NSButton`s that forward to the hidden stock buttons, so close vetoes,
//! minimize, full screen and the window's accessibility relationships stay
//! native. This crate is the workspace's only `unsafe` boundary; it is confined
//! to the `macos` module.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{Error, WindowControls};

/// One app-owned window control, in leading-to-trailing order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// Close the window, through its delegate (a veto still applies).
    Close,
    /// Minimize the window to the Dock.
    Minimize,
    /// Enter or leave full screen, as the stock green button does.
    FullScreen,
}

/// The controls in the order they are laid out.
pub const CONTROLS: [Control; 3] = [Control::Close, Control::Minimize, Control::FullScreen];

impl Control {
    /// Accessible name and tooltip.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Close => "Close",
            Self::Minimize => "Minimize",
            Self::FullScreen => "Full Screen",
        }
    }

    /// SF Symbol drawn for the control.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Close => "xmark",
            Self::Minimize => "minus",
            Self::FullScreen => "arrow.up.left.and.arrow.down.right",
        }
    }
}

/// Placement of the control group, in points from the window's top-leading corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Distance from the leading edge to the first control.
    pub leading: f64,
    /// Distance from the top edge to each control.
    pub top: f64,
    /// Width and height of each control's hit area.
    pub size: f64,
    /// Space between neighbouring controls.
    pub gap: f64,
}

impl Layout {
    /// Hit area of each control, large enough for a pointer and a hover backing.
    pub const SIZE: f64 = 24.;
    /// Space between controls.
    pub const GAP: f64 = 2.;
    /// Distance from the leading window edge, matching the header's padding.
    pub const LEADING: f64 = 16.;

    /// The group vertically centred in a titlebar band `titlebar` points tall.
    #[must_use]
    pub fn centered(titlebar: f64) -> Self {
        Self {
            leading: Self::LEADING,
            top: ((titlebar - Self::SIZE) / 2.).max(0.).round(),
            size: Self::SIZE,
            gap: Self::GAP,
        }
    }

    /// Distance from the leading edge to the end of the last control.
    #[must_use]
    pub fn end(&self) -> f64 {
        (self.size + self.gap).mul_add(3., self.leading - self.gap)
    }

    /// Each control's `[x, y, width, height]` inside a container `height` points tall.
    /// A flipped container's y grows downward; an unflipped one's grows upward.
    #[must_use]
    pub fn frames(&self, height: f64, flipped: bool) -> [[f64; 4]; 3] {
        let y = if flipped {
            self.top
        } else {
            height - self.top - self.size
        };
        let mut frames = [[0.; 4]; 3];
        let mut x = self.leading;
        for frame in &mut frames {
            *frame = [x, y, self.size, self.size];
            x += self.size + self.gap;
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_is_centred_in_the_band_and_ends_after_three_controls() {
        let layout = Layout::centered(52.);
        assert_eq!(
            layout,
            Layout {
                leading: 16.,
                top: 14.,
                size: 24.,
                gap: 2.
            }
        );
        assert!((layout.end() - 92.).abs() < f64::EPSILON);
    }

    #[test]
    fn frames_follow_the_container_orientation() {
        let layout = Layout::centered(52.);
        assert_eq!(
            layout.frames(720., true),
            [
                [16., 14., 24., 24.],
                [42., 14., 24., 24.],
                [68., 14., 24., 24.]
            ]
        );
        assert_eq!(
            layout.frames(720., false),
            [
                [16., 682., 24., 24.],
                [42., 682., 24., 24.],
                [68., 682., 24., 24.]
            ]
        );
    }

    #[test]
    fn controls_have_distinct_labels_and_symbols() {
        assert_eq!(
            CONTROLS.map(Control::label),
            ["Close", "Minimize", "Full Screen"]
        );
        assert_eq!(
            CONTROLS.map(Control::symbol),
            ["xmark", "minus", "arrow.up.left.and.arrow.down.right"]
        );
    }
}
