//! What a published snapshot marks on each server for this account (see
//! [`crate::marked`]), kept so unchanged servers and marks are shared.

use std::sync::{Arc, LazyLock};

use crate::{
    marked::{ChannelMarks, MarkedServer, ServerMarks},
    notifications::Notifications,
    ready::{Channel, Server},
    unread::Unreads,
};

/// A server as a snapshot published it: made from `source`, for an account
/// timed out there until `timeout`.
pub(super) struct Published {
    pub(super) source: Arc<Server>,
    pub(super) timeout: Option<u64>,
    /// `source`, or while a timeout lasts a copy of it that carries the
    /// timeout, with its marks.
    pub(super) listed: MarkedServer,
}

/// Whether `channel` is a thread gone quiet at `now`, in Unix milliseconds.
fn quiet(channel: &Channel, now: u64) -> bool {
    channel
        .thread
        .as_deref()
        .is_some_and(|thread| thread.quiet_at(&channel.id) <= now)
}

impl ServerMarks {
    /// What [`super::tracker::Tracker::snapshot`] marks on `server` for
    /// this account, `me`, with `roles` there.
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
        // A voice channel's chat is read as a text channel is.
        for channel in server.text_channels.iter().chain(&server.voice_channels) {
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
                opened: false,
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
                opened: unreads.opened(&thread.id),
            });
        }
        let (suppress_everyone, suppress_roles) = unreads.suppressed(&server.id);
        Self {
            muted: unreads.server_muted(&server.id),
            roles: roles.to_vec(),
            notifications: settings(server, unreads),
            suppress_everyone,
            suppress_roles,
            channels,
        }
    }

    /// These marks, made from `server`, with the unread state of its
    /// channels and threads, and which threads are quiet, as they are at
    /// `now`; none when that's all as it was, so nothing is copied.
    pub(super) fn unread(&self, server: &Server, unreads: &Unreads, now: u64) -> Option<Self> {
        let mut changed: Option<Vec<ChannelMarks>> = None;
        // In the order of `new`: text channels, voice channels, threads.
        let channels = server
            .text_channels
            .iter()
            .chain(&server.voice_channels)
            .chain(&server.threads)
            .zip(&self.channels)
            .enumerate();
        for (at, (channel, was)) in channels {
            let mut marks = *was;
            (marks.muted, marks.unread, marks.mentions) =
                unreads.channel(&server.id, channel, was.access);
            marks.last_read = unreads.last_read(&server.id, &channel.id);
            marks.quiet = quiet(channel, now);
            marks.opened = channel.thread.is_some() && unreads.opened(&channel.id);
            if marks != *was {
                changed.get_or_insert_with(|| self.channels.clone())[at] = marks;
            }
        }
        let muted = unreads.server_muted(&server.id);
        let notifications = settings(server, unreads);
        let (suppress_everyone, suppress_roles) = unreads.suppressed(&server.id);
        let same = changed.is_none()
            && muted == self.muted
            && notifications == self.notifications
            && (suppress_everyone, suppress_roles) == (self.suppress_everyone, self.suppress_roles);
        (!same).then(|| Self {
            muted,
            roles: self.roles.clone(),
            notifications,
            suppress_everyone,
            suppress_roles,
            channels: changed.unwrap_or_else(|| self.channels.clone()),
        })
    }
}

/// The account's notification settings for `server`, the defaults until
/// Discord sends them.
fn settings(server: &Server, unreads: &Unreads) -> Arc<Notifications> {
    static DEFAULTS: LazyLock<Arc<Notifications>> = LazyLock::new(Arc::default);
    unreads.settings(&server.id).unwrap_or(&DEFAULTS).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_made_again_only_when_they_change() {
        let channel = |id: &str| Channel {
            id: id.into(),
            ..Default::default()
        };
        let server = Server {
            id: "10".into(),
            text_channels: vec![channel("11"), channel("12")],
            voice_channels: vec![channel("13")],
            ..Default::default()
        };
        let unreads = Unreads::default();
        let marks = ServerMarks::new(&server, "1", &["20".into()], &unreads, 0);
        assert_eq!(marks.channels.len(), 3);
        assert_eq!(marks.unread(&server, &unreads, 0), None);
        // Read since: only that channel's marks change.
        let mut unread = marks.clone();
        unread.channels[1].unread = true;
        let read = unread.unread(&server, &unreads, 0).unwrap();
        assert_eq!(read, marks);
        assert_eq!(read.roles, ["20"]);
    }
}
