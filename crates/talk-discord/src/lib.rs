//! Talk's Discord connection: account login, the gateway, messages and voice.
//! See docs/discord.md. Everything Discord-specific stays in this crate.

pub mod api;
pub mod cache;
pub mod client;
pub mod cookies;
mod dave;
pub mod embed;
pub mod event;
pub mod gateway;
pub mod limits;
pub mod member_list;
pub mod message;
pub mod notifications;
pub mod profile;
pub mod ready;
pub mod remote_auth;
pub mod rtp;
pub mod search;
pub mod service;
pub mod settings;
pub mod stream;
pub mod stream_connection;
pub mod token;
pub mod unread;
pub mod voice;
