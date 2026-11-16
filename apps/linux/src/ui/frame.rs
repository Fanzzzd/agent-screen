//! Converts `wlr-screencopy` `RawRgbaFrame` buffers into BGRA `gpui::RenderImage`
//! textures and reclaims superseded textures via `Window::drop_image` so
//! continuous live streaming never leaks GPU sprite-atlas memory.

use agent_screen_app_core::ports::RawRgbaFrame;
use gpui::{RenderImage, Window};
use image::{Frame, RgbaImage};
use smallvec::smallvec;
use std::sync::Arc;

/// Convert an RGBA8888 capture buffer into GPUI's expected BGRA8888 `RenderImage`.
pub fn raw_to_render_image(raw: &RawRgbaFrame) -> Option<Arc<RenderImage>> {
    if raw.width == 0 || raw.height == 0 {
        return None;
    }
    let expected = (raw.width as usize)
        .checked_mul(raw.height as usize)?
        .checked_mul(4)?;
    if raw.rgba.len() < expected {
        return None;
    }
    let mut bgra = raw.rgba[..expected].to_vec();
    rgba_to_bgra_in_place(&mut bgra);
    let buf = RgbaImage::from_raw(raw.width, raw.height, bgra)?;
    let frame = Frame::new(buf);
    Some(Arc::new(RenderImage::new(smallvec![frame])))
}

pub fn rgba_to_bgra_in_place(bytes: &mut [u8]) {
    for px in bytes.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
}

/// Holds at most one GPU-resident `RenderImage` for a view and drops superseded
/// textures explicitly on swap or teardown.
#[derive(Default)]
pub struct FrameSlot {
    current: Option<(u64, Arc<RenderImage>)>,
    pub uploaded_frames: u64,
    pub dropped_frames: u64,
}

impl FrameSlot {
    pub fn image(&self) -> Option<Arc<RenderImage>> {
        self.current.as_ref().map(|(_, img)| img.clone())
    }

    pub fn resident_textures(&self) -> u64 {
        self.uploaded_frames.saturating_sub(self.dropped_frames)
    }

    /// Replace the current image if `raw.digest` changed, calling
    /// `window.drop_image` on the previous `RenderImage` first.
    pub fn swap(&mut self, raw: &RawRgbaFrame, window: &mut Window) -> bool {
        if self
            .current
            .as_ref()
            .is_some_and(|(digest, _)| *digest == raw.digest)
        {
            return false;
        }
        let Some(next) = raw_to_render_image(raw) else {
            return false;
        };
        if let Some((_, prev)) = self.current.take() {
            let _ = window.drop_image(prev);
            self.dropped_frames += 1;
        }
        self.uploaded_frames += 1;
        self.current = Some((raw.digest, next));
        true
    }

    pub fn clear(&mut self, window: &mut Window) {
        if let Some((_, prev)) = self.current.take() {
            let _ = window.drop_image(prev);
            self.dropped_frames += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_swaps_red_and_blue_into_bgra_preserving_green_and_alpha() {
        let mut buf = vec![220, 40, 15, 255, 10, 200, 90, 128];
        rgba_to_bgra_in_place(&mut buf);
        assert_eq!(buf, vec![15, 40, 220, 255, 90, 200, 10, 128]);
    }
}
