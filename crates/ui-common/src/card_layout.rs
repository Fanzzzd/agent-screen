//! Shared Help Card geometry and aspect-fit thumbnail sizing.

pub const WIDTH: f32 = 320.;
pub const PADDING: f32 = 14.;
pub const GAP: f32 = 8.;
pub const TITLE: f32 = 20.;
pub const REASON: f32 = 36.;
pub const LABEL: f32 = 16.;
pub const BUTTONS: f32 = 30.;
pub const MAX_PICTURE: f32 = 184.;
pub const RADIUS: f32 = 18.;

pub const REQUEST_HEIGHT: f32 = PADDING * 2. + TITLE + REASON + LABEL + BUTTONS + GAP * 3.;

pub fn height(size: (f64, f64), answers: bool) -> f32 {
    let rows = if answers { 2. } else { 1. };
    PADDING * 2. + TITLE + REASON + LABEL + BUTTONS * rows + GAP * (3. + rows) + picture(size).1
}

pub fn picture(size: (f64, f64)) -> (f32, f32) {
    let w = size.0.max(1.0) as f32;
    let h = size.1.max(1.0) as f32;
    let width = WIDTH - PADDING * 2.;
    let fit = (width / w).min(MAX_PICTURE / h);
    (w * fit, h * fit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_picture_fits_within_card_bounds() {
        let (pw, ph) = picture((1280.0, 800.0));
        assert!(pw <= WIDTH - PADDING * 2. + 0.01);
        assert!(ph <= MAX_PICTURE + 0.01);
        assert!(height((1280.0, 800.0), true) > height((1280.0, 800.0), false));
    }
}
