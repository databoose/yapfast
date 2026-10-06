//! Talk's Discord connection: account login, the gateway, messages and voice.
//! See docs/discord.md. Everything Discord-specific stays in this crate.

pub mod api;
pub mod client;
pub mod gateway;
pub mod message;
pub mod ready;
pub mod service;
pub mod token;
pub mod unread;
pub mod voice;
