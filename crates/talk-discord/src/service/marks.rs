//! What a published snapshot marks on each server for this account: access,
//! mutes and unread state, kept so unchanged servers are shared.

use std::sync::Arc;

use crate::{
    ready::{Access, Server},
    unread::Unreads,
};

/// A server as a snapshot published it: marked from `source` with `marks`,
/// for an account with `roles` there.
pub(super) struct Published {
    pub(super) source: Arc<Server>,
    pub(super) roles: Vec<String>,
    pub(super) marks: Marks,
    pub(super) marked: Arc<Server>,
}

/// What [`super::tracker::Tracker::snapshot`] sets on a server for this account: whether
/// it's muted, its roles there and which mentions its settings suppress, then
/// for each text channel and each voice channel, in order, what
/// [`ChannelMarks`] holds.
#[derive(PartialEq, Eq)]
pub(super) struct Marks {
    muted: bool,
    roles: Vec<String>,
    suppressed: (bool, bool),
    channels: Vec<ChannelMarks>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ChannelMarks {
    access: Access,
    mention_everyone: bool,
    muted: bool,
    unread: bool,
    mentions: u64,
    last_read: u64,
}

impl Marks {
    pub(super) fn new(server: &Server, me: &str, roles: &[String], unreads: &Unreads) -> Self {
        let mut channels =
            Vec::with_capacity(server.text_channels.len() + server.voice_channels.len());
        for channel in &server.text_channels {
            let access = server.access(channel, me, roles);
            let (muted, unread, mentions) = unreads.channel(&server.id, channel, access);
            channels.push(ChannelMarks {
                access,
                mention_everyone: server.may_mention_everyone(channel, me, roles),
                muted,
                unread,
                mentions,
                last_read: unreads.last_read(&server.id, &channel.id),
            });
        }
        for channel in &server.voice_channels {
            channels.push(ChannelMarks {
                access: server.access(channel, me, roles),
                mention_everyone: server.may_mention_everyone(channel, me, roles),
                muted: channel.muted,
                unread: channel.unread,
                mentions: channel.mentions,
                last_read: channel.last_read,
            });
        }
        Self {
            muted: unreads.server_muted(&server.id),
            roles: roles.to_vec(),
            suppressed: unreads.suppressed(&server.id),
            channels,
        }
    }

    /// These marks with the unread state of `server`'s text channels as
    /// it is now.
    pub(super) fn unread(&self, server: &Server, unreads: &Unreads) -> Self {
        let mut channels = self.channels.clone();
        for (channel, marks) in server.text_channels.iter().zip(&mut channels) {
            (marks.muted, marks.unread, marks.mentions) =
                unreads.channel(&server.id, channel, marks.access);
            marks.last_read = unreads.last_read(&server.id, &channel.id);
        }
        Self {
            muted: unreads.server_muted(&server.id),
            roles: self.roles.clone(),
            suppressed: unreads.suppressed(&server.id),
            channels,
        }
    }

    /// A copy of `server` with these marks.
    pub(super) fn apply(&self, server: &Server) -> Server {
        let mut marked = server.clone();
        marked.muted = self.muted;
        marked.my_roles.clone_from(&self.roles);
        (marked.suppress_everyone, marked.suppress_roles) = self.suppressed;
        let channels = marked
            .text_channels
            .iter_mut()
            .chain(&mut marked.voice_channels);
        for (channel, marks) in channels.zip(&self.channels) {
            channel.access = marks.access;
            channel.mention_everyone = marks.mention_everyone;
            channel.muted = marks.muted;
            channel.unread = marks.unread;
            channel.mentions = marks.mentions;
            channel.last_read = marks.last_read;
        }
        marked
    }
}
