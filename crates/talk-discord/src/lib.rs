//! Talk's Discord connection: account login, the gateway, messages and voice.
//! See docs/discord.md. Everything Discord-specific stays in this crate.

pub mod api;
pub mod client;
mod dave;
pub mod embed;
pub mod gateway;
pub mod member_list;
pub mod message;
pub mod profile;
pub mod ready;
pub mod rtp;
pub mod service;
pub mod stream;
pub mod stream_connection;
pub mod token;
pub mod unread;
pub mod voice;
