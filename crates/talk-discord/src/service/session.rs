//! The gateway session: connecting, then answering Talk's requests and
//! taking in Discord's events until the connection ends, and reconnecting.

use std::{
    pin::Pin,
    sync::{Arc, RwLock},
};

use anyhow::{Result, anyhow};
use serde_json::Value;
use tokio::{
    sync::{broadcast, mpsc, watch},
    time::{Duration, Instant, Sleep},
};

use super::{
    Live, Request, Snapshot, Status, live::live_events, member_lists::MemberLists,
    member_search::MemberSearches, now_ms, publisher::Publisher, read_acks::ReadAcks, server,
    stream_openings::StreamOpenings, tracker::Tracker, voice_join::VoiceJoin,
};
use crate::{
    api::Api,
    event::Event,
    gateway::{Closed, Dispatch, Gateway, Reconnects, TooManyIdentifies},
    message::{self, Names},
    ready,
};

/// What outlasts a session: the handle's channels and what Talk has on screen.
pub(super) struct Shared {
    pub(super) snapshots: watch::Sender<Arc<Snapshot>>,
    pub(super) live: broadcast::Sender<Live>,
    pub(super) names: Arc<RwLock<Names>>,
    pub(super) api: Arc<Api>,
    pub(super) lists: MemberLists,
    /// The presence this session last sent, which it identifies with again.
    pub(super) presence: Value,
}

pub(super) async fn run(
    token: String,
    mut requests: mpsc::UnboundedReceiver<Request>,
    mut shared: Shared,
) {
    let snapshots = shared.snapshots.clone();
    let mut reconnects = Reconnects::default();
    loop {
        snapshots.send_modify(|snapshot| Arc::make_mut(snapshot).status = Status::Connecting);
        let connected = tokio::select! {
            connected = Gateway::connect(&token, shared.presence.clone(), &mut reconnects) => connected,
            () = snapshots.closed() => return,
        };
        let (error, up) = match connected {
            Ok((gateway, ready)) => {
                let started = tokio::time::Instant::now();
                let ended = connected_session(gateway, ready, &mut requests, &mut shared).await;
                shared.lists.disconnected();
                match ended {
                    Ended::Dropped => return,
                    Ended::Failed(error) => (error, Some(started.elapsed())),
                }
            }
            Err(error) => (error, None),
        };
        tracing::warn!(%error, "Discord connection ended");
        let stopped = if error.downcast_ref::<Closed>().is_some_and(Closed::is_final) {
            Some("Discord refused the saved login. Link your Discord account again.")
        } else if error.downcast_ref::<TooManyIdentifies>().is_some() {
            Some(
                "Discord keeps ending the connection, so Talk stopped reconnecting. Restart Talk to try again.",
            )
        } else {
            None
        };
        if let Some(reason) = stopped {
            snapshots.send_modify(|snapshot| {
                let snapshot = Arc::make_mut(snapshot);
                snapshot.status = Status::Offline(reason.into());
                snapshot.voice.clear();
            });
            // Keep answering until Talk drops the handles.
            while let Some(request) = requests.recv().await {
                refuse(request, &mut shared);
            }
            return;
        }
        snapshots.send_modify(|snapshot| {
            let snapshot = Arc::make_mut(snapshot);
            snapshot.status = Status::Offline("Can't reach Discord. Reconnecting...".into());
            snapshot.voice.clear();
        });
        let wait = tokio::time::sleep(reconnects.wait(&error, up));
        tokio::pin!(wait);
        loop {
            tokio::select! {
                () = &mut wait => break,
                request = requests.recv() => match request {
                    Some(request) => refuse(request, &mut shared),
                    None => return,
                },
            }
        }
    }
}

/// Answers a request while disconnected. Viewing a channel is remembered.
fn refuse(request: Request, shared: &mut Shared) {
    match request {
        Request::Join { reply, .. } => {
            let _ = reply.send(Err(anyhow!(
                "Discord isn't connected right now. Try again in a moment."
            )));
        }
        Request::View { guild, channel } => {
            shared.lists.view_offline(&shared.snapshots, guild, channel);
        }
        _ => {}
    }
}

enum Ended {
    /// Every handle was dropped.
    Dropped,
    Failed(anyhow::Error),
}

async fn connected_session(
    gateway: Gateway,
    ready: Value,
    requests: &mut mpsc::UnboundedReceiver<Request>,
    shared: &mut Shared,
) -> Ended {
    let Some(me) = ready::user_id(&ready).map(str::to_owned) else {
        return Ended::Failed(anyhow!("Discord's READY had no user"));
    };
    let mut tracker = Tracker::from_ready(me, &ready, shared.names.clone());
    // READY can contain tens of megabytes of JSON. Only the parsed account
    // state is needed while connected; release the payload before waiting.
    drop(ready);
    trim_startup_heap();
    tracker.keep_unavailable(&shared.snapshots.borrow().servers);
    shared.snapshots.send_replace(Arc::new(tracker.snapshot()));
    let mut session = Session {
        gateway,
        publisher: Publisher::new(shared.snapshots.clone()),
        acks: ReadAcks::new(shared.api.clone()),
        status_ends: Box::pin(tokio::time::sleep(status_left(&tracker))),
        tracker,
        shared,
        join: VoiceJoin::new(),
        streams: StreamOpenings::new(),
        searches: MemberSearches::default(),
    };
    if let Err(error) = session.connect().await {
        return Ended::Failed(error);
    }
    session.run(requests).await
}

/// glibc otherwise holds freed READY allocations for the entire session.
/// Trim once after parsing, before the session starts handling requests.
fn trim_startup_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: malloc_trim only returns unused allocator pages to the OS.
        unsafe { malloc_trim(0) };
    }
}

/// A connected session, and what it waits on.
struct Session<'a> {
    gateway: Gateway,
    tracker: Tracker,
    shared: &'a mut Shared,
    publisher: Publisher,
    join: VoiceJoin,
    streams: StreamOpenings,
    searches: MemberSearches,
    acks: ReadAcks,
    /// When the custom status ends by itself.
    status_ends: Pin<Box<Sleep>>,
}

impl Session<'_> {
    async fn connect(&mut self) -> Result<()> {
        // This session shows the status the settings hold, as Discord's own apps do.
        self.show_presence().await?;
        self.shared.lists.publish_group(&self.tracker);
        self.shared
            .lists
            .connect(&self.tracker, &mut self.gateway)
            .await
    }

    async fn run(mut self, requests: &mut mpsc::UnboundedReceiver<Request>) -> Ended {
        loop {
            let done = tokio::select! {
                () = self.publisher.due(), if self.publisher.stale() => {
                    self.publisher.publish(&mut self.tracker, &self.shared.lists);
                    Ok(())
                }
                request = requests.recv() => match request {
                    Some(request) => self.request(request).await,
                    None => {
                        self.gateway.close().await;
                        return Ended::Dropped;
                    }
                },
                () = self.status_ends.as_mut(), if self.tracker.custom_status.as_ref().is_some_and(|custom| custom.expires_at_ms.is_some()) => {
                    self.tracker.custom_status = None;
                    self.publisher.changed();
                    self.show_presence().await
                }
                () = self.shared.lists.went_stale(), if self.shared.lists.unsent() => {
                    // The list on screen went stale without Discord sending it.
                    self.shared.lists.refresh(&mut self.gateway).await
                }
                () = self.acks.send_when_due(), if self.acks.waiting() => Ok(()),
                () = self.streams.timed_out(), if self.streams.waiting() => Ok(()),
                () = self.join.timed_out(), if self.join.waiting() => Ok(()),
                event = self.gateway.next() => match event {
                    Ok(event) => self.event(event).await,
                    Err(error) => {
                        self.join.fail("Discord disconnected while joining voice.");
                        self.streams.fail("Discord disconnected while opening the stream.");
                        Err(error)
                    }
                },
            };
            if let Err(error) = done {
                return Ended::Failed(error);
            }
        }
    }

    /// Acts on a request from the handle; an error ends the session.
    async fn request(&mut self, request: Request) -> Result<()> {
        let gateway = &mut self.gateway;
        match request {
            Request::Join {
                guild,
                channel,
                reply,
            } => {
                let sent = gateway
                    .update_voice_state(server(&guild), Some(&channel), true, false)
                    .await;
                self.join.start(guild, channel, reply);
                sent
            }
            Request::Leave { guild } => {
                self.join.leave(&guild);
                gateway
                    .update_voice_state(server(&guild), None, false, false)
                    .await
            }
            Request::SelfState {
                guild,
                channel,
                mute,
                deaf,
            } => {
                gateway
                    .update_voice_state(server(&guild), Some(&channel), mute, deaf)
                    .await
            }
            Request::View { guild, channel } => {
                let listed = self
                    .shared
                    .lists
                    .view(&self.tracker, gateway, guild, channel)
                    .await?;
                if listed {
                    self.publisher.changed();
                }
                Ok(())
            }
            Request::MarkRead {
                guild,
                channel,
                sync,
            } => {
                if let Some(message) = self.tracker.unreads.mark_read(&guild, &channel) {
                    self.publisher.changed();
                    if sync {
                        self.acks.read(channel, message);
                    }
                }
                Ok(())
            }
            Request::GoLive {
                guild,
                channel,
                reply,
            } => {
                self.streams
                    .go_live(gateway, &self.tracker, guild, channel, reply)
                    .await
            }
            Request::Watch { stream_key, reply } => {
                self.streams
                    .watch(gateway, &self.tracker, stream_key, reply)
                    .await
            }
            Request::EndStream { stream_key } => self.streams.end(gateway, &stream_key).await,
            Request::PauseStream { stream_key, paused } => {
                gateway.pause_stream(&stream_key, paused).await
            }
            Request::CustomStatus(custom) => {
                // The settings update that follows says the same; this
                // shows it without waiting.
                self.tracker.custom_status = custom;
                self.publisher.changed();
                self.custom_status_changed();
                self.show_presence().await
            }
            Request::Status(status) => {
                self.tracker.status = status;
                self.publisher.changed();
                self.show_presence().await
            }
            Request::Search {
                guild,
                query,
                reply,
            } => self.searches.start(gateway, &guild, &query, reply).await,
            Request::Members { guild, users } => {
                // Discord answers at most 100 users per request.
                for users in users.chunks(100) {
                    gateway.request_members(&guild, users).await?;
                }
                Ok(())
            }
        }
    }

    /// Takes in a dispatched event; an error ends the session.
    async fn event(&mut self, dispatch: Dispatch) -> Result<()> {
        let (event, data) = (&dispatch.event, &dispatch.data);
        // A server update that no join or stream waits for moves one
        // under way.
        let awaited = match event {
            Event::VoiceServerUpdate => self.join.awaits(data),
            Event::StreamServerUpdate => self.streams.awaits(data),
            _ => true,
        };
        self.join.apply(&self.tracker.me, &dispatch);
        // Whether the tracker already took this event in, and changed.
        let mut applied = None;
        match event {
            Event::PresenceUpdate => self.shared.lists.presence(&self.tracker, data),
            Event::GuildMemberListUpdate => {
                if self.shared.lists.update(&self.tracker, data) {
                    self.publisher.changed();
                }
            }
            Event::GuildMembersChunk => {
                applied = self.searches.answer(&mut self.tracker, &dispatch);
            }
            _ => {}
        }
        // Parsed once, for the timeline and for a notification.
        let message = matches!(event, Event::MessageCreate | Event::MessageUpdate)
            .then(|| message::parse(data))
            .flatten();
        let me = &self.tracker.me;
        for live in live_events(me, &self.shared.names, event, data, message.as_ref()) {
            let _ = self.shared.live.send(live);
        }
        if *event == Event::MessageCreate
            && self.tracker.notifies(data)
            && let Some(message) = message
        {
            let guild = data["guild_id"].as_str().map(str::to_owned);
            let _ = self.shared.live.send(Live::Notify { guild, message });
        }
        if applied.unwrap_or_else(|| self.tracker.apply(event, data)) {
            self.publisher.changed();
        }
        if !awaited && let Some(moved) = self.tracker.moved(event, data) {
            let _ = self.shared.live.send(moved);
        }
        if *event == Event::UserSettingsProtoUpdate {
            self.custom_status_changed();
            self.show_presence().await?;
        }
        self.shared.lists.group_event(&self.tracker, &dispatch);
        if self.streams.waiting() {
            if event.is_stream() {
                self.streams.apply(&self.tracker, &dispatch);
            } else if *event == Event::VoiceStateUpdate {
                // This account's voice session may be what a stream waited for.
                self.streams.voice_changed(&self.tracker);
            }
        }
        Ok(())
    }

    /// Sends the gateway this session's presence when it changed.
    async fn show_presence(&mut self) -> Result<()> {
        let presence = self.tracker.own_presence();
        if presence != self.shared.presence {
            self.gateway.update_presence(presence.clone()).await?;
            self.shared.presence = presence;
        }
        Ok(())
    }

    /// Waits for the custom status as it is now to end by itself.
    fn custom_status_changed(&mut self) {
        self.status_ends
            .as_mut()
            .reset(Instant::now() + status_left(&self.tracker));
    }
}

/// How long until the custom status ends by itself; long when it doesn't.
fn status_left(tracker: &Tracker) -> Duration {
    tracker
        .custom_status
        .as_ref()
        .and_then(|custom| custom.expires_at_ms)
        .map_or(Duration::from_secs(86_400 * 365), |at| {
            Duration::from_millis(at.saturating_sub(now_ms()))
        })
}
