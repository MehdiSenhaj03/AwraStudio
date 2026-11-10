//! Awra Studio's AI sound engineer: tools over the studio, engineering
//! know-how, an offline rule-based engineer, and language model providers.
//!
//! SPDX-License-Identifier: GPL-3.0-or-later

pub mod agent;
pub mod engineer;
pub mod host;
pub mod local;
pub mod tools;

pub use agent::{run_turn, ChatMessage, ProviderConfig, ProviderKind, TurnResult};
pub use host::{LayoutCommand, PluginDesc, StudioHost};
