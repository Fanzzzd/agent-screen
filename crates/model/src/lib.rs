//! Agent Screen's domain rules, shared by the app on every platform: Agent
//! Displays and their lifecycle, window records and fates, key chords, and
//! the DevTools pipe's drags.
//! Nothing here touches the OS.

pub mod access;
pub mod alerts;
pub mod arrangement;
pub mod budget;
pub mod claims;
pub mod devtools;
pub mod input_queue;
pub mod keychord;
pub mod launch;
pub mod model;
pub mod numbers;
pub mod recording_playback;
pub mod substitute;
pub mod tree;
pub mod typing;

pub mod diagnostics;
pub mod peek;
pub mod pointer_geometry;
