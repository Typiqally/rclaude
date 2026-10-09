//! Isolated Claude remote controller. Provider credentials never enter its broker protocol.

pub mod protocol;

pub mod auth;
pub mod client;

pub mod broker;
pub mod stream;

pub mod history;
