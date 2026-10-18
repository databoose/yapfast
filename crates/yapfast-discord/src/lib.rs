//! Yapfast's Discord connection: account login, the gateway, messages and voice.
//! See docs/discord.md. Everything Discord-specific stays in this crate.

pub mod activity;
pub mod api;
pub mod cache;
mod call_connection;
pub mod call_stats;
pub mod call_video;
pub mod captcha;
pub mod client;
pub mod cookies;
mod dave;
pub mod embed;
pub mod event;
pub mod forum;
pub mod frecency;
pub mod gateway;
pub mod gifs;
pub mod limits;
pub mod member_list;
pub mod message;
pub mod notifications;
pub mod opus;
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
mod voice_connection;
