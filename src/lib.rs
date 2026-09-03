//! dancefloor's internals, exposed as a library so the render path can be
//! driven by tests without a terminal.

pub mod app;
pub mod config;
pub mod discovery;
pub mod model;
pub mod settings;
pub mod subagents;
pub mod transcript;
pub mod ui;
