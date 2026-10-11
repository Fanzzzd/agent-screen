//! macOS: CGVirtualDisplay, ScreenCaptureKit, SkyLight input, Accessibility.

pub mod aerospace;
pub mod app;
pub mod assist;
pub mod ax;
pub mod capture;
pub mod catch;
pub mod cg;
pub mod display;
pub mod input;
pub mod menubar;
pub mod native;
pub mod notify;
pub mod permissions;
pub mod screenshot;
pub mod viewer_input;
pub mod windows;

pub use native::menu_bar_height as menubar_height;
