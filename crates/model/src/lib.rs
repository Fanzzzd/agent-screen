//! Agent Screen's domain rules, shared by the app on every platform: Agent
//! Displays and their lifecycle, window records and fates, key chords, and
//! the DevTools pipe's drags.
//! Nothing here touches the OS.

pub mod access;
pub mod alerts;
pub mod arrangement;
pub mod budget;
pub mod card;
pub mod claims;
pub mod devtools;
pub mod held;
pub mod input_queue;
pub mod key_window;
pub mod keychord;
pub mod launch;
pub mod menu_press;
pub mod model;
pub mod new_window;
pub mod numbers;
pub mod placement;
pub mod real_click;
pub mod real_drag;
pub mod recording_playback;
pub mod request;
pub mod returns;
pub mod screen_edge;
pub mod selection;
pub mod session;
pub mod shared_app;
pub mod spares;
pub mod substitute;
pub mod tree;
pub mod typing;
pub mod viewer_keys;
pub mod viewer_press;
pub mod watch;

pub mod diagnostics;
pub mod peek;
pub mod pointer_geometry;
