//! Awra Studio's audio processing: buffers, file I/O, built-in effects
//! (EQ, dynamics, reverb, delay, autotune, denoise...), loudness, and the
//! mix graph shared by real-time playback and offline rendering.
//!
//! SPDX-License-Identifier: GPL-3.0-or-later

pub mod autotune;
pub mod buffer;
pub mod denoise;
pub mod effects;
pub mod filters;
pub mod graph;
pub mod io;
pub mod loudness;
pub mod pitch;

pub use buffer::{AudioBuffer, Peaks};
pub use effects::Processor;
pub use graph::{AssetStore, MixGraph};
