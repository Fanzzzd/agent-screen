//! Colors and the icon set. Light and dark follow the system appearance.

use gpui::{
    App, AssetSource, Bounds, Hsla, IntoElement, PathBuilder, Pixels, Result, SharedString, Styled,
    Window, WindowAppearance, canvas, point, px, rgb, rgba,
};
use std::borrow::Cow;
use std::cell::Cell;

#[derive(Clone, Copy)]
pub struct Theme {
    /// The card behind the Viewer and Peek.
    pub card: Hsla,
    pub card_border: Hsla,
    pub label: Hsla,
    pub secondary: Hsla,
    /// Resting fill of a plain button.
    pub control: Hsla,
    pub control_hover: Hsla,
    pub accent: Hsla,
    pub orange: Hsla,
    pub red: Hsla,
    /// Behind a display image that has no frame yet.
    pub screen_empty: Hsla,
    /// Covers the user's menu bar on an Agent Display's picture.
    pub menu_strip: Hsla,
    /// The dimmed layer under a sheet.
    pub scrim: Hsla,
    pub sheet: Hsla,
}

/// The palettes, built from hex values.
pub fn palette(dark: bool) -> Theme {
    if dark {
        Theme {
            card: rgb(0x1e1e20).into(),
            card_border: rgba(0xffffff26).into(),
            label: rgb(0xf5f5f7).into(),
            secondary: rgb(0x98989d).into(),
            control: rgba(0xffffff1f).into(),
            control_hover: rgba(0xffffff33).into(),
            accent: rgb(0x0a84ff).into(),
            orange: rgb(0xff9f0a).into(),
            red: rgb(0xff453a).into(),
            screen_empty: rgba(0x00000059).into(),
            menu_strip: rgb(0x323236).into(),
            scrim: rgba(0x00000073).into(),
            sheet: rgba(0x2c2c2ef8).into(),
        }
    } else {
        Theme {
            card: rgb(0xf6f6f8).into(),
            card_border: rgba(0x0000001f).into(),
            label: rgb(0x1d1d1f).into(),
            secondary: rgb(0x6e6e73).into(),
            control: rgba(0x0000000f).into(),
            control_hover: rgba(0x0000001f).into(),
            accent: rgb(0x007aff).into(),
            orange: rgb(0xff9500).into(),
            red: rgb(0xff3b30).into(),
            // Darker than the + tile, so an empty screen never reads as one.
            screen_empty: rgba(0x00000059).into(),
            menu_strip: rgb(0xe8e8ed).into(),
            scrim: rgba(0x00000059).into(),
            sheet: rgba(0xfbfbfdf8).into(),
        }
    }
}

thread_local! {
    /// The user's accent color for light and dark, when the system has one.
    static ACCENT: Cell<Option<(Hsla, Hsla)>> = const { Cell::new(None) };
}

/// Follow the user's accent color, given as sRGB for light and dark.
pub fn set_accent(accent: Option<([f32; 3], [f32; 3])>) {
    let hsla = |[r, g, b]: [f32; 3]| Hsla::from(gpui::Rgba { r, g, b, a: 1.0 });
    ACCENT.with(|a| a.set(accent.map(|(light, dark)| (hsla(light), hsla(dark)))));
}

pub fn theme(window: &Window) -> Theme {
    let dark = matches!(
        window.appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    let mut theme = palette(dark);
    if let Some((light, night)) = ACCENT.with(Cell::get) {
        theme.accent = if dark { night } else { light };
    }
    theme
}

/// Icons, compiled in.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            "icons/plus.svg" => include_bytes!("../../assets/icons/plus.svg"),
            "icons/close.svg" => include_bytes!("../../assets/icons/close.svg"),
            "icons/chevron-up.svg" => include_bytes!("../../assets/icons/chevron-up.svg"),
            "icons/play.svg" => include_bytes!("../../assets/icons/play.svg"),
            "icons/hand.svg" => include_bytes!("../../assets/icons/hand.svg"),
            "icons/tray.svg" => include_bytes!("../../assets/icons/tray.svg"),
            "icons/alert.svg" => include_bytes!("../../assets/icons/alert.svg"),
            "icons/windows.svg" => include_bytes!("../../assets/icons/windows.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

/// Rounds the corners of a live surface. GPUI clips surfaces to rectangles
/// only, so this paints the four corners outside the rounded shape in the
/// color around it.
pub fn round_corners(radius: f32, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _: &mut App| {
            let r = radius;
            let (l, t) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (rr, b) = (
                l + f32::from(bounds.size.width),
                t + f32::from(bounds.size.height),
            );
            // Each corner: the corner point and the center of its arc, plus
            // the quarter of the circle that faces the corner.
            let corners = [
                ((l, t), (l + r, t + r), 180.0f32),
                ((rr, t), (rr - r, t + r), 270.0),
                ((rr, b), (rr - r, b - r), 0.0),
                ((l, b), (l + r, b - r), 90.0),
            ];
            for (corner, center, start) in corners {
                let mut path = PathBuilder::fill();
                path.move_to(point(px(corner.0), px(corner.1)));
                for step in 0..=8 {
                    let angle = (start + step as f32 * 90.0 / 8.0).to_radians();
                    path.line_to(point(
                        px(center.0 + r * angle.cos()),
                        px(center.1 + r * angle.sin()),
                    ));
                }
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}
