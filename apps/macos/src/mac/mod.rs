//! macOS: CGVirtualDisplay, ScreenCaptureKit, SkyLight input, Accessibility.

pub mod activity;
pub mod aerospace;
pub mod alerts;
pub mod app;
pub mod armed_hold;
pub mod assist;
pub mod ax;
pub mod capture;
pub mod catch;
pub mod cg;
pub mod claims;
pub mod click_diagnostics;
pub mod colorsync;
pub mod devtools;
pub mod display;
pub mod hang;
pub mod hotkey;
pub mod ime;
pub mod input;
pub mod key_hold;
pub mod menubar;
pub mod mux;
pub mod native;
pub mod notify;
pub mod permissions;
pub mod playback;
pub mod real_input;
pub mod recording;
pub mod relocate;
#[cfg(test)]
mod scan_bench;
pub mod screenshot;
pub mod shared_app;
pub mod spaces;
pub mod update;
pub mod viewer_input;
pub mod windows;

pub use native::menu_bar_height as menubar_height;
