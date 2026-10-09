//! dancefloor's internals, exposed as a library so the render path can be
//! driven by tests without a terminal.

pub mod app;
pub mod clipboard;
pub mod config;
pub mod digest;
pub mod discovery;
pub mod editor;
pub mod model;
pub mod process;
pub mod providers;
pub mod settings;
pub mod subagents;
pub mod transcript;
pub mod ui;
