//! macOS: CGVirtualDisplay, ScreenCaptureKit, SkyLight input, Accessibility.

pub mod aerospace;
pub mod alerts;
pub mod app;
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
pub mod ime;
pub mod input;
pub mod menubar;
pub mod mux;
pub mod native;
pub mod notify;
pub mod permissions;
pub mod playback;
pub mod recording;
pub mod screenshot;
pub mod update;
pub mod viewer_input;
pub mod windows;

pub use native::menu_bar_height as menubar_height;
