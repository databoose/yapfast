//! What a published snapshot marks on each server for this account: access,
//! mutes and unread state, kept so unchanged servers are shared.

use std::sync::Arc;

use crate::{
    notifications::Notifications,
    ready::{Access, Channel, Server},
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
/// it's muted, its roles there, its notification settings and which mentions
/// they suppress, then
/// for each text channel, each voice channel and each thread, in order,
/// what [`ChannelMarks`] holds.
#[derive(PartialEq, Eq)]
pub(super) struct Marks {
    muted: bool,
    roles: Vec<String>,
    /// Shared with the tracker's, and compared by address first.
    settings: Arc<Notifications>,
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
    /// A thread gone quiet; see [`crate::ready::Thread::quiet_at`].
    quiet: bool,
}

/// Whether `channel` is a thread gone quiet at `now`, in Unix milliseconds.
fn quiet(channel: &Channel, now: u64) -> bool {
    channel
        .thread
        .as_deref()
        .is_some_and(|thread| thread.quiet_at(&channel.id) <= now)
}

impl Marks {
    pub(super) fn new(
        server: &Server,
        me: &str,
        roles: &[String],
        unreads: &Unreads,
        now: u64,
    ) -> Self {
        let mut channels = Vec::with_capacity(
            server.text_channels.len() + server.voice_channels.len() + server.threads.len(),
        );
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
                quiet: false,
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
                quiet: false,
            });
        }
        for thread in &server.threads {
            let access = server.access(thread, me, roles);
            let (muted, unread, mentions) = unreads.channel(&server.id, thread, access);
            channels.push(ChannelMarks {
                access,
                mention_everyone: server.may_mention_everyone(thread, me, roles),
                muted,
                unread,
                mentions,
                last_read: unreads.last_read(&server.id, &thread.id),
                quiet: quiet(thread, now),
            });
        }
        Self {
            muted: unreads.server_muted(&server.id),
            roles: roles.to_vec(),
            settings: settings(server, unreads),
            suppressed: unreads.suppressed(&server.id),
            channels,
        }
    }

    /// These marks, made from `server`, with the unread state of its text
    /// channels and threads, and which threads are quiet, as they are at
    /// `now`.
    pub(super) fn unread(&self, server: &Server, unreads: &Unreads, now: u64) -> Self {
        let mut channels = self.channels.clone();
        // Text channels come first, and threads after the voice channels.
        let (text, rest) = channels.split_at_mut(server.text_channels.len());
        let threads = &mut rest[server.voice_channels.len()..];
        let written = server
            .text_channels
            .iter()
            .zip(text)
            .chain(server.threads.iter().zip(threads));
        for (channel, marks) in written {
            (marks.muted, marks.unread, marks.mentions) =
                unreads.channel(&server.id, channel, marks.access);
            marks.last_read = unreads.last_read(&server.id, &channel.id);
            marks.quiet = quiet(channel, now);
        }
        Self {
            muted: unreads.server_muted(&server.id),
            roles: self.roles.clone(),
            settings: settings(server, unreads),
            suppressed: unreads.suppressed(&server.id),
            channels,
        }
    }

    /// A copy of `server` with these marks.
    pub(super) fn apply(&self, server: &Server) -> Server {
        let mut marked = server.clone();
        marked.muted = self.muted;
        marked.my_roles.clone_from(&self.roles);
        marked.notifications = self.settings.clone();
        (marked.suppress_everyone, marked.suppress_roles) = self.suppressed;
        let channels = marked
            .text_channels
            .iter_mut()
            .chain(&mut marked.voice_channels)
            .chain(&mut marked.threads);
        for (channel, marks) in channels.zip(&self.channels) {
            channel.access = marks.access;
            channel.mention_everyone = marks.mention_everyone;
            channel.muted = marks.muted;
            channel.unread = marks.unread;
            channel.mentions = marks.mentions;
            channel.last_read = marks.last_read;
            if let Some(thread) = channel.thread.as_deref_mut() {
                thread.quiet = marks.quiet;
            }
        }
        marked
    }
}

/// The account's notification settings for `server`, the defaults until
/// Discord sends them.
fn settings(server: &Server, unreads: &Unreads) -> Arc<Notifications> {
    unreads
        .settings(&server.id)
        .cloned()
        .unwrap_or_else(|| server.notifications.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ready::{Emoji, Overwrite};

    #[test]
    fn a_marked_server_shares_its_roles_emoji_and_overwrites() {
        let server = Server {
            id: "10".into(),
            roles: Arc::new([("10".into(), 0)].into()),
            role_names: Arc::new([("20".into(), "Mods".into())].into()),
            emojis: Arc::new(vec![Emoji {
                id: "5".into(),
                name: "wave".into(),
                animated: false,
                available: true,
            }]),
            text_channels: vec![Channel {
                id: "11".into(),
                overwrites: Arc::new(vec![Overwrite {
                    id: "10".into(),
                    member: false,
                    allow: 0,
                    deny: 0,
                }]),
                ..Default::default()
            }],
            ..Default::default()
        };
        let marks = Marks::new(&server, "1", &[], &Unreads::default(), 0);
        let marked = marks.apply(&server);
        assert!(Arc::ptr_eq(&marked.roles, &server.roles));
        assert!(Arc::ptr_eq(&marked.role_names, &server.role_names));
        assert!(Arc::ptr_eq(&marked.emojis, &server.emojis));
        assert!(Arc::ptr_eq(
            &marked.text_channels[0].overwrites,
            &server.text_channels[0].overwrites
        ));
    }
}
