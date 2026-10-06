//! Talk's Discord connection: account login, the gateway and voice.
//! See docs/discord.md. Everything Discord-specific stays in this crate.

pub mod api;
pub mod client;
pub mod gateway;
pub mod ready;
pub mod token;
pub mod voice;
