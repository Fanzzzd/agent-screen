//! Agent Screen's own desktop picture for an Agent Display: drawn by the
//! app at the display's size, with the display's name, so System Settings,
//! which shows each display with its desktop, tells it apart from the
//! user's monitors. The app draws it, so it needs no file in the bundle and
//! keeps working wherever the app runs from.

/// The picture's background, sRGB 0-255: the purple Agent Displays had.
pub const BACKGROUND: (u8, u8, u8) = (69, 61, 111);

/// Where the two lines go on a picture `width` × `height` pixels, in
/// pixels with the origin at the bottom left, as AppKit draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// The display's name, the big line.
    pub title_size: f64,
    /// Center of the title line, from the bottom.
    pub title_center: f64,
    /// The line under it.
    pub tagline_size: f64,
    pub tagline_center: f64,
    /// Widest a line may be; the app shrinks a line drawn wider.
    pub max_width: f64,
}

/// Sizes that keep "Agent Screen e2e 12" on one line on a wide display and
/// on a 640 × 1600 portrait one: the title is at most a fourteenth of the
/// width and a ninth of the height.
pub fn layout(width: f64, height: f64) -> Layout {
    let title_size = (width / 14.0).min(height / 9.0).max(1.0);
    let tagline_size = title_size * 0.34;
    // Slightly above the middle, as the eye reads a centered pair.
    let title_center = height * 0.55;
    let tagline_center = title_center - title_size * 0.95;
    Layout {
        title_size,
        title_center,
        tagline_size,
        tagline_center,
        max_width: width * 0.86,
    }
}

/// The font size that fits text measured `measured` wide at `size` into
/// `max_width`: `size` when it fits.
pub fn fit(size: f64, measured: f64, max_width: f64) -> f64 {
    if measured <= max_width || measured <= 0.0 {
        size
    } else {
        size * max_width / measured
    }
}

/// The picture's file name for the screen called `name`, `width` ×
/// `height` pixels, in `lang`. Each differs in its name: macOS skips
/// setting a picture whose URL a screen already shows, and a display that
/// changed size or language needs a new one.
pub fn file_name(name: &str, width: u32, height: u32, lang: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{} {width}x{height} {lang} v1.png", safe.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_display_sizes_by_width() {
        let l = layout(1280.0, 1024.0);
        assert!((l.title_size - 1280.0 / 14.0).abs() < 1e-9);
        assert!(l.tagline_size < l.title_size);
        assert!(l.tagline_center < l.title_center);
        assert!(l.tagline_center - l.tagline_size > 0.0);
    }

    #[test]
    fn wide_display_sizes_by_height() {
        let l = layout(2560.0, 1600.0);
        assert!((l.title_size - 1600.0 / 9.0).abs() < 1e-9);
    }

    #[test]
    fn portrait_display_keeps_both_lines_inside() {
        let l = layout(1280.0, 3200.0);
        assert!(l.title_center + l.title_size / 2.0 < 3200.0);
        assert!(l.tagline_center - l.tagline_size / 2.0 > 0.0);
        assert!(l.max_width < 1280.0);
    }

    #[test]
    fn fit_shrinks_only_lines_too_wide() {
        assert_eq!(fit(100.0, 500.0, 800.0), 100.0);
        assert_eq!(fit(100.0, 1600.0, 800.0), 50.0);
        assert_eq!(fit(100.0, 0.0, 800.0), 100.0);
    }

    #[test]
    fn file_names_differ_by_name_size_and_language() {
        let a = file_name("Agent Screen 2", 2560, 1600, "en");
        assert_eq!(a, "Agent Screen 2 2560x1600 en v1.png");
        assert_ne!(a, file_name("Agent Screen 3", 2560, 1600, "en"));
        assert_ne!(a, file_name("Agent Screen 2", 1280, 800, "en"));
        assert_ne!(a, file_name("Agent Screen 2", 2560, 1600, "zh"));
    }

    #[test]
    fn file_names_have_no_path_separators() {
        let n = file_name("../a/b:c", 10, 10, "en");
        assert!(!n.contains('/') && !n.contains(':'));
    }
}
