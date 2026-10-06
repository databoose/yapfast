//! Talk's Discord connection: one gateway session kept open while Talk runs.
//! It tracks the account's servers, their channels and who is in voice,
//! passes on live message events, and hands the voice server details for a
//! join to the voice worker.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::{Result, anyhow};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::{
    api::Api,
    gateway::{Closed, Gateway},
    member_list::{ListedMember, MemberList},
    message::{self, Message, Names},
    ready::{self, Server},
    stream::{self, Opening, StreamServer},
    unread::Unreads,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Connecting,
    Online,
    /// Disconnected, with a reason a person can act on.
    Offline(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceMember {
    pub user_id: String,
    pub name: String,
    /// The address of the avatar Discord shows for them on this server.
    pub avatar: String,
    pub muted: bool,
    pub deafened: bool,
    /// Sharing their screen with Go Live.
    pub streaming: bool,
}

/// The linked account, as Discord shows it outside any server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    /// The address of its avatar.
    pub avatar: String,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub status: Status,
    pub user_id: Option<String>,
    pub account: Option<Account>,
    pub servers: Vec<Server>,
    /// Who is in each voice channel, by channel id and then by name.
    pub voice: HashMap<String, Vec<VoiceMember>>,
    /// The channel whose member list is on screen, and how many people can
    /// read it.
    pub listed: Option<(String, u64)>,
}

/// A message event from the gateway, for any channel.
#[derive(Clone, Debug)]
pub enum Live {
    Created(Message),
    Updated(Message),
    Deleted {
        channel: String,
        id: String,
    },
    Reaction {
        channel: String,
        message: String,
        key: String,
        emoji: String,
        me: bool,
        added: bool,
    },
    /// Every reaction, or every one with `key`, was removed.
    ReactionsCleared {
        channel: String,
        message: String,
        key: Option<String>,
    },
    /// Names were learned, so mentions and authors may render differently.
    Names,
    /// The member list of the channel last viewed, as it stands now.
    Members {
        channel: String,
        members: Vec<ListedMember>,
    },
    /// Who watches a stream this account is in, and whether it's paused.
    StreamUpdated {
        stream_key: String,
        viewers: Vec<String>,
        paused: bool,
    },
    /// A stream this account shared or watched ended, with Discord's reason
    /// such as "user_requested" or "stream_full".
    StreamEnded {
        stream_key: String,
        reason: Option<String>,
    },
}

/// What songbird needs to open the voice connection Discord assigned.
#[derive(Clone)]
pub struct VoiceServer {
    pub user_id: String,
    pub session_id: String,
    pub token: String,
    pub endpoint: String,
}

enum Request {
    Join {
        guild: String,
        channel: String,
        reply: oneshot::Sender<Result<VoiceServer>>,
    },
    Leave {
        guild: String,
    },
    SelfState {
        guild: String,
        channel: String,
        mute: bool,
        deaf: bool,
    },
    View {
        guild: String,
        channel: Option<String>,
    },
    Members {
        guild: String,
        users: Vec<String>,
    },
    MarkRead {
        guild: String,
        channel: String,
        sync: bool,
    },
    GoLive {
        guild: String,
        channel: String,
        reply: oneshot::Sender<Result<StreamServer>>,
    },
    Watch {
        stream_key: String,
        reply: oneshot::Sender<Result<StreamServer>>,
    },
    EndStream {
        stream_key: String,
    },
    PauseStream {
        stream_key: String,
        paused: bool,
    },
}

/// A handle to the connection. It ends when every handle is dropped.
#[derive(Clone)]
pub struct Discord {
    requests: mpsc::UnboundedSender<Request>,
    snapshots: watch::Receiver<Snapshot>,
    live: broadcast::Sender<Live>,
    api: Arc<Api>,
    names: Arc<RwLock<Names>>,
}

const LIVE_BACKLOG: usize = 256;

impl Discord {
    /// Connects on the current Tokio runtime and keeps reconnecting until
    /// Discord rejects the token.
    pub fn start(token: String) -> Result<Self> {
        let api = Arc::new(Api::new(token.clone())?);
        let (requests, receiver) = mpsc::unbounded_channel();
        let (snapshots, watcher) = watch::channel(Snapshot::default());
        let (live, _) = broadcast::channel(LIVE_BACKLOG);
        let names = Arc::new(RwLock::new(Names::default()));
        tokio::spawn(run(
            token,
            receiver,
            Shared {
                snapshots,
                live: live.clone(),
                names: names.clone(),
                api: api.clone(),
                viewing: HashSet::new(),
                listing: None,
                lists: Vec::new(),
            },
        ));
        Ok(Self {
            requests,
            snapshots: watcher,
            live,
            api,
            names,
        })
    }

    pub fn snapshots(&self) -> watch::Receiver<Snapshot> {
        self.snapshots.clone()
    }

    /// Message events for every channel, from now on.
    pub fn live(&self) -> broadcast::Receiver<Live> {
        self.live.subscribe()
    }

    /// Tells Discord a server's channel is on screen, so its messages arrive
    /// live. With a text `channel`, its member list arrives as [`Live::Members`].
    pub fn view(&self, guild: &str, channel: Option<&str>) {
        let _ = self.requests.send(Request::View {
            guild: guild.into(),
            channel: channel.map(str::to_owned),
        });
    }

    /// Marks a channel read in Talk, and on Discord too with `sync`.
    pub fn mark_read(&self, guild: &str, channel: &str, sync: bool) {
        let _ = self.requests.send(Request::MarkRead {
            guild: guild.into(),
            channel: channel.into(),
            sync,
        });
    }

    /// A page of messages before `before`, or the latest page, oldest first.
    pub async fn history(
        &self,
        guild: &str,
        channel: &str,
        before: Option<&str>,
    ) -> Result<Vec<Message>> {
        let values = self.api.messages(channel, before).await?;
        let mut unknown = HashSet::new();
        {
            let mut names = self.write_names();
            for value in &values {
                names.learn_message(Some(guild), value);
                if let Some(author) = value["author"]["id"].as_str()
                    && !names.knows_member(guild, author)
                {
                    unknown.insert(author.to_owned());
                }
            }
        }
        // History carries no server profiles, so nicknames come separately.
        if !unknown.is_empty() {
            let _ = self.requests.send(Request::Members {
                guild: guild.into(),
                users: unknown.into_iter().collect(),
            });
        }
        let mut messages: Vec<Message> = values.iter().filter_map(message::parse).collect();
        messages.reverse();
        Ok(messages)
    }

    /// Sends a message, or a reply to `reply` in the same channel, with a
    /// file given as its name and contents.
    pub async fn send(
        &self,
        guild: &str,
        channel: &str,
        content: &str,
        nonce: &str,
        reply: Option<&str>,
        file: Option<(String, Vec<u8>)>,
    ) -> Result<Message> {
        let upload = match file {
            Some((name, data)) => Some(self.api.upload(channel, &name, data).await?),
            None => None,
        };
        let sent = self
            .api
            .send(Some(guild), channel, content, nonce, reply, upload.as_ref())
            .await;
        if let (Err(_), Some(upload)) = (&sent, &upload) {
            let _ = self.api.discard(upload).await;
        }
        let value = sent?;
        self.write_names().learn_message(Some(guild), &value);
        message::parse(&value).ok_or_else(|| anyhow!("could not understand Discord's answer"))
    }

    pub async fn edit(&self, channel: &str, id: &str, content: &str) -> Result<Message> {
        let value = self.api.edit(channel, id, content).await?;
        message::parse(&value).ok_or_else(|| anyhow!("could not understand Discord's answer"))
    }

    pub async fn delete(&self, channel: &str, id: &str) -> Result<()> {
        self.api.delete(channel, id).await
    }

    /// Adds or removes this account's reaction; `emoji` is in the API's form.
    pub async fn react(&self, channel: &str, id: &str, emoji: &str, add: bool) -> Result<()> {
        self.api.react(channel, id, emoji, add).await
    }

    /// Uploads the preview Discord shows of this account's live stream: a
    /// JPEG as a data URI.
    pub async fn stream_preview(&self, stream_key: &str, thumbnail: &str) -> Result<()> {
        self.api.stream_preview(stream_key, thumbnail).await
    }

    pub async fn download(&self, url: &str, limit: usize) -> Result<Vec<u8>> {
        self.api.download(url, limit).await
    }

    /// A user's name in a server: their nickname, display name or username.
    pub fn name(&self, guild: &str, user: &str) -> Option<String> {
        self.read_names().name(guild, user).map(str::to_owned)
    }

    /// The address of a user's avatar, in a server or in DMs.
    pub fn avatar(&self, guild: Option<&str>, user: &str) -> String {
        self.read_names().avatar(guild, user)
    }

    /// Discord Markdown for Talk, with mentions, custom emoji and times as text.
    pub fn render(&self, server: Option<&Server>, content: &str) -> String {
        self.read_names().render(server, content)
    }

    fn read_names(&self) -> std::sync::RwLockReadGuard<'_, Names> {
        self.names
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_names(&self) -> std::sync::RwLockWriteGuard<'_, Names> {
        self.names
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Asks Discord to join a voice channel, muted like every Talk call at
    /// first, and waits for the voice server it assigns.
    pub async fn join(&self, guild: &str, channel: &str) -> Result<VoiceServer> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Request::Join {
                guild: guild.into(),
                channel: channel.into(),
                reply,
            })
            .map_err(|_| anyhow!("Discord isn't connected. Restart Talk and try again."))?;
        answer
            .await
            .map_err(|_| anyhow!("Discord disconnected while joining voice."))?
    }

    pub fn leave(&self, guild: &str) {
        let _ = self.requests.send(Request::Leave {
            guild: guild.into(),
        });
    }

    /// Shows others whether this account is muted or deafened.
    pub fn set_self(&self, guild: &str, channel: &str, mute: bool, deaf: bool) {
        let _ = self.requests.send(Request::SelfState {
            guild: guild.into(),
            channel: channel.into(),
            mute,
            deaf,
        });
    }

    /// Starts sharing to the voice channel this account is in, and waits for
    /// the server Discord assigns the stream.
    pub async fn go_live(&self, guild: &str, channel: &str) -> Result<StreamServer> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Request::GoLive {
                guild: guild.into(),
                channel: channel.into(),
                reply,
            })
            .map_err(|_| anyhow!("Discord isn't connected. Restart Talk and try again."))?;
        answer
            .await
            .map_err(|_| anyhow!("Discord disconnected while starting the stream."))?
    }

    /// Starts watching a stream in the voice channel this account is in.
    pub async fn watch(&self, stream_key: &str) -> Result<StreamServer> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Request::Watch {
                stream_key: stream_key.into(),
                reply,
            })
            .map_err(|_| anyhow!("Discord isn't connected. Restart Talk and try again."))?;
        answer
            .await
            .map_err(|_| anyhow!("Discord disconnected while opening the stream."))?
    }

    /// Ends this account's stream, or stops watching someone else's.
    pub fn end_stream(&self, stream_key: &str) {
        let _ = self.requests.send(Request::EndStream {
            stream_key: stream_key.into(),
        });
    }

    pub fn pause_stream(&self, stream_key: &str, paused: bool) {
        let _ = self.requests.send(Request::PauseStream {
            stream_key: stream_key.into(),
            paused,
        });
    }
}

const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
/// Reads within this long go to Discord as one acknowledgement per channel.
const ACK_DELAY: Duration = Duration::from_secs(1);
const FIRST_RETRY: Duration = Duration::from_secs(2);
const LAST_RETRY: Duration = Duration::from_secs(60);

struct Shared {
    snapshots: watch::Sender<Snapshot>,
    live: broadcast::Sender<Live>,
    names: Arc<RwLock<Names>>,
    api: Arc<Api>,
    /// Servers with a channel on screen, subscribed again after reconnecting.
    viewing: HashSet<String>,
    /// The server and channel whose member list is on screen.
    listing: Option<(String, String)>,
    /// Member lists of channels shown lately, newest last, to show again at
    /// once while Discord sends them afresh.
    lists: Vec<MemberList>,
}

/// Member lists kept for channels shown again.
const LISTS: usize = 16;

impl Shared {
    /// Keeps a member list Discord sent, for when its channel is shown again.
    fn keep_list(&mut self, list: MemberList) {
        if !list.synced() {
            return;
        }
        self.lists.retain(|kept| kept.channel() != list.channel());
        self.lists.push(list);
        if self.lists.len() > LISTS {
            self.lists.remove(0);
        }
    }

    /// The member list kept for a channel, or an empty one.
    fn take_list(&mut self, guild: &str, channel: &str) -> MemberList {
        match self
            .lists
            .iter()
            .position(|list| list.guild() == guild && list.channel() == channel)
        {
            Some(index) => {
                let mut list = self.lists.remove(index);
                list.revisit();
                list
            }
            None => MemberList::new(guild.to_owned(), channel.to_owned()),
        }
    }
}

async fn run(token: String, mut requests: mpsc::UnboundedReceiver<Request>, mut shared: Shared) {
    let snapshots = shared.snapshots.clone();
    let mut retry = FIRST_RETRY;
    loop {
        snapshots.send_modify(|snapshot| snapshot.status = Status::Connecting);
        let connected = tokio::select! {
            connected = Gateway::connect(&token) => connected,
            () = snapshots.closed() => return,
        };
        let error = match connected {
            Ok((gateway, ready)) => {
                retry = FIRST_RETRY;
                match connected_session(gateway, &ready, &mut requests, &mut shared).await {
                    Ended::Dropped => return,
                    Ended::Failed(error) => error,
                }
            }
            Err(error) => error,
        };
        let closed = error.downcast_ref::<Closed>();
        tracing::warn!(%error, "Discord connection ended");
        if closed.is_some_and(Closed::is_final) {
            snapshots.send_modify(|snapshot| {
                snapshot.status = Status::Offline(
                    "Discord refused the saved login. Link your Discord account again.".into(),
                );
                snapshot.voice.clear();
            });
            // Keep answering until Talk drops the handles.
            while let Some(request) = requests.recv().await {
                refuse(request, &mut shared);
            }
            return;
        }
        snapshots.send_modify(|snapshot| {
            snapshot.status = Status::Offline("Can't reach Discord. Reconnecting...".into());
            snapshot.voice.clear();
        });
        let wait = tokio::time::sleep(retry);
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
        retry = (retry * 2).min(LAST_RETRY);
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
            if let Some(channel) = channel {
                shared.listing = Some((guild.clone(), channel));
            }
            shared.viewing.insert(guild);
        }
        _ => {}
    }
}

/// Passes a stream dispatch to the streams being opened, and answers those
/// that are now ready or that Discord refused.
fn open_streams(
    openings: &mut HashMap<String, PendingStream>,
    tracker: &Tracker,
    name: &str,
    data: &Value,
) {
    let Some(key) = data["stream_key"].as_str() else {
        return;
    };
    let Some(mut waiting) = openings.remove(key) else {
        return;
    };
    if name == "STREAM_DELETE" {
        let reason = match data["reason"].as_str() {
            Some("stream_full") => "The stream is full.",
            Some("unauthorized") => "You aren't allowed to stream or watch here.",
            _ => "The stream ended before it started.",
        };
        let _ = waiting.reply.send(Err(anyhow!(reason)));
        return;
    }
    waiting.opening.apply(key, name, data);
    let session = tracker.voice_session(&waiting.guild);
    match session.and_then(|session| waiting.opening.server(key, &tracker.me, session)) {
        Some(server) => {
            let _ = waiting.reply.send(Ok(server));
        }
        None => {
            openings.insert(key.to_owned(), waiting);
        }
    }
}

enum Ended {
    /// Every handle was dropped.
    Dropped,
    Failed(anyhow::Error),
}

struct Pending {
    guild: String,
    reply: oneshot::Sender<Result<VoiceServer>>,
    session_id: Option<String>,
    server: Option<(String, String)>,
}

/// A stream started or watched, waiting for its server.
struct PendingStream {
    guild: String,
    reply: oneshot::Sender<Result<StreamServer>>,
    opening: Opening,
}

fn fail_streams(openings: &mut HashMap<String, PendingStream>, message: &str) {
    for (_, waiting) in openings.drain() {
        let _ = waiting.reply.send(Err(anyhow!("{message}")));
    }
}

async fn connected_session(
    mut gateway: Gateway,
    ready: &Value,
    requests: &mut mpsc::UnboundedReceiver<Request>,
    shared: &mut Shared,
) -> Ended {
    let Some(me) = ready::user_id(ready).map(str::to_owned) else {
        return Ended::Failed(anyhow!("Discord's READY had no user"));
    };
    let mut tracker = Tracker::from_ready(me.clone(), ready, shared.names.clone());
    shared.snapshots.send_replace(tracker.snapshot());
    tracker.list = shared
        .listing
        .clone()
        .map(|(guild, channel)| shared.take_list(&guild, &channel));
    for guild in &shared.viewing {
        let channel = tracker
            .list
            .as_ref()
            .filter(|list| list.guild() == guild)
            .map(MemberList::channel);
        if let Err(error) = gateway.subscribe(guild, channel).await {
            return Ended::Failed(error);
        }
    }
    let mut pending: Option<Pending> = None;
    let deadline = tokio::time::sleep(JOIN_TIMEOUT);
    tokio::pin!(deadline);
    let mut openings: HashMap<String, PendingStream> = HashMap::new();
    let stream_deadline = tokio::time::sleep(JOIN_TIMEOUT);
    tokio::pin!(stream_deadline);
    // The newest read message per channel, waiting to be acknowledged.
    let mut acks: HashMap<String, String> = HashMap::new();
    let ack_at = tokio::time::sleep(ACK_DELAY);
    tokio::pin!(ack_at);
    loop {
        tokio::select! {
            request = requests.recv() => {
                let Some(request) = request else {
                    gateway.close().await;
                    return Ended::Dropped;
                };
                let sent = match request {
                    Request::Join { guild, channel, reply } => {
                        let sent = gateway.update_voice_state(Some(&guild), Some(&channel), true, false).await;
                        deadline.as_mut().reset(tokio::time::Instant::now() + JOIN_TIMEOUT);
                        if let Some(previous) = pending.replace(Pending { guild, reply, session_id: None, server: None }) {
                            let _ = previous.reply.send(Err(anyhow!("Another voice join replaced this one.")));
                        }
                        sent
                    }
                    Request::Leave { guild } => {
                        if pending.as_ref().is_some_and(|pending| pending.guild == guild) {
                            pending = None;
                        }
                        gateway.update_voice_state(Some(&guild), None, false, false).await
                    }
                    Request::SelfState { guild, channel, mute, deaf } => {
                        gateway.update_voice_state(Some(&guild), Some(&channel), mute, deaf).await
                    }
                    Request::View { guild, channel } => {
                        let new = shared.viewing.insert(guild.clone());
                        match channel {
                            Some(channel) if shared.listing.as_ref().is_none_or(|(g, c)| *g != guild || *c != channel) => {
                                shared.listing = Some((guild.clone(), channel.clone()));
                                let list = shared.take_list(&guild, &channel);
                                if let Some(previous) = tracker.list.replace(list) {
                                    shared.keep_list(previous);
                                }
                                // Shown at once from the cache, until Discord's sync.
                                if tracker.list.as_ref().is_some_and(MemberList::synced) {
                                    publish_members(&tracker, shared);
                                }
                                shared.snapshots.send_replace(tracker.snapshot());
                                gateway.subscribe(&guild, Some(&channel)).await
                            }
                            // The same channel again: what Discord sent still stands.
                            Some(_) => {
                                if tracker.list.as_ref().is_some_and(MemberList::synced) {
                                    publish_members(&tracker, shared);
                                }
                                Ok(())
                            }
                            None if new => gateway.subscribe(&guild, None).await,
                            None => Ok(()),
                        }
                    }
                    Request::MarkRead { guild, channel, sync } => {
                        if let Some(message) = tracker.unreads.mark_read(&guild, &channel) {
                            shared.snapshots.send_replace(tracker.snapshot());
                            if sync {
                                if acks.is_empty() {
                                    ack_at.as_mut().reset(tokio::time::Instant::now() + ACK_DELAY);
                                }
                                acks.insert(channel, message);
                            }
                        }
                        Ok(())
                    }
                    Request::GoLive { guild, channel, reply } => {
                        if tracker.voice_channel(&guild).is_none_or(|current| current != channel) {
                            let _ = reply.send(Err(anyhow!("Join the voice channel before going live.")));
                            Ok(())
                        } else {
                            let key = stream::guild_key(&guild, &channel, &me);
                            stream_deadline.as_mut().reset(tokio::time::Instant::now() + JOIN_TIMEOUT);
                            let waiting = PendingStream { guild: guild.clone(), reply, opening: Opening::default() };
                            if let Some(previous) = openings.insert(key.clone(), waiting) {
                                let _ = previous.reply.send(Err(anyhow!("Going live again replaced this attempt.")));
                            }
                            // Unpaused straight away, as Discord-video-stream
                            // does: viewers of a paused stream get no video.
                            match gateway.create_stream(&guild, &channel).await {
                                Ok(()) => gateway.pause_stream(&key, false).await,
                                Err(error) => Err(error),
                            }
                        }
                    }
                    Request::Watch { stream_key, reply } => {
                        match stream::parse(&stream_key) {
                            Some((guild, channel, _)) if tracker.voice_channel(guild) == Some(channel) => {
                                let guild = guild.to_owned();
                                stream_deadline.as_mut().reset(tokio::time::Instant::now() + JOIN_TIMEOUT);
                                let waiting = PendingStream { guild, reply, opening: Opening::default() };
                                if let Some(previous) = openings.insert(stream_key.clone(), waiting) {
                                    let _ = previous.reply.send(Err(anyhow!("Watching again replaced this attempt.")));
                                }
                                gateway.watch_stream(&stream_key).await
                            }
                            Some(_) => {
                                let _ = reply.send(Err(anyhow!("Join the voice channel to watch its streams.")));
                                Ok(())
                            }
                            None => {
                                let _ = reply.send(Err(anyhow!("Talk can only watch streams in server voice channels.")));
                                Ok(())
                            }
                        }
                    }
                    Request::EndStream { stream_key } => {
                        if let Some(waiting) = openings.remove(&stream_key) {
                            let _ = waiting.reply.send(Err(anyhow!("The stream was stopped before it started.")));
                        }
                        gateway.delete_stream(&stream_key).await
                    }
                    Request::PauseStream { stream_key, paused } => {
                        gateway.pause_stream(&stream_key, paused).await
                    }
                    Request::Members { guild, users } => {
                        // Discord answers at most 100 users per request.
                        let mut sent = Ok(());
                        for users in users.chunks(100) {
                            sent = gateway.request_members(&guild, users).await;
                            if sent.is_err() { break; }
                        }
                        sent
                    }
                };
                if let Err(error) = sent {
                    return Ended::Failed(error);
                }
            }
            () = &mut ack_at, if !acks.is_empty() => {
                for (channel, message) in acks.drain() {
                    let api = shared.api.clone();
                    tokio::spawn(async move {
                        if let Err(error) = api.ack(&channel, &message).await {
                            tracing::warn!(%error, "could not mark a Discord channel read");
                        }
                    });
                }
            }
            () = &mut stream_deadline, if !openings.is_empty() => {
                fail_streams(&mut openings, "Discord didn't assign a stream server. Try again.");
            }
            () = &mut deadline, if pending.is_some() => {
                if let Some(pending) = pending.take() {
                    let _ = pending.reply.send(Err(anyhow!("Discord didn't assign a voice server. Try again.")));
                }
            }
            event = gateway.next() => {
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        if let Some(pending) = pending.take() {
                            let _ = pending.reply.send(Err(anyhow!("Discord disconnected while joining voice.")));
                        }
                        fail_streams(&mut openings, "Discord disconnected while opening the stream.");
                        return Ended::Failed(error);
                    }
                };
                if let Some(waiting) = pending.as_mut() {
                    match event.name.as_str() {
                        "VOICE_STATE_UPDATE" if event.data["user_id"] == me.as_str() && event.data["guild_id"] == waiting.guild.as_str() => {
                            waiting.session_id = event.data["session_id"].as_str().map(str::to_owned);
                        }
                        // The endpoint is null while Discord is still allocating a server.
                        "VOICE_SERVER_UPDATE" if event.data["guild_id"] == waiting.guild.as_str() => {
                            if let (Some(token), Some(endpoint)) = (event.data["token"].as_str(), event.data["endpoint"].as_str()) {
                                waiting.server = Some((token.into(), endpoint.into()));
                            }
                        }
                        _ => {}
                    }
                    if let (Some(session_id), Some((token, endpoint))) = (&waiting.session_id, &waiting.server) {
                        let server = VoiceServer { user_id: me.clone(), session_id: session_id.clone(), token: token.clone(), endpoint: endpoint.clone() };
                        if let Some(done) = pending.take() {
                            let _ = done.reply.send(Ok(server));
                        }
                    }
                }
                if event.name == "PRESENCE_UPDATE"
                    && tracker.list.as_mut().is_some_and(|list| list.presence(&event.data))
                {
                    publish_members(&tracker, shared);
                }
                if event.name == "GUILD_MEMBER_LIST_UPDATE"
                    && let Some(list) = tracker.list.as_mut()
                {
                    let count = list.count();
                    let changed = list.apply(&event.data, &mut shared.names.write().unwrap_or_else(|poisoned| poisoned.into_inner()));
                    let counted = list.count() != count;
                    if changed {
                        publish_members(&tracker, shared);
                    }
                    if counted {
                        shared.snapshots.send_replace(tracker.snapshot());
                    }
                }
                for live in live_events(&me, &shared.names, &event.name, &event.data) {
                    let _ = shared.live.send(live);
                }
                if tracker.apply(&event.name, &event.data) {
                    shared.snapshots.send_replace(tracker.snapshot());
                }
                if !openings.is_empty() && event.name.starts_with("STREAM_") {
                    open_streams(&mut openings, &tracker, &event.name, &event.data);
                }
            }
        }
    }
}

struct VoiceEntry {
    channel: String,
    /// The voice session, which a stream connection reuses.
    session: Option<String>,
    muted: bool,
    deafened: bool,
    streaming: bool,
}

/// The account's servers and voice channels, kept current from gateway events.
struct Tracker {
    me: String,
    servers: Vec<Server>,
    names: Arc<RwLock<Names>>,
    /// Where each user is in voice, by server and user id.
    voice: HashMap<(String, String), VoiceEntry>,
    /// This account's roles, by server id.
    my_roles: HashMap<String, Vec<String>>,
    unreads: Unreads,
    /// The member list of the channel on screen.
    list: Option<MemberList>,
}

impl Tracker {
    fn from_ready(me: String, ready: &Value, names: Arc<RwLock<Names>>) -> Self {
        let mut tracker = Self {
            me,
            servers: ready::servers(ready),
            names,
            voice: HashMap::new(),
            my_roles: HashMap::new(),
            unreads: Unreads::from_ready(ready),
            list: None,
        };
        tracker.learn_user(&ready["user"]);
        for user in ready["users"].as_array().into_iter().flatten() {
            tracker.learn_user(user);
        }
        tracker.learn_members(&ready["guilds"], &ready["merged_members"]);
        tracker
    }

    /// Applies one dispatched event; returns whether anything visible changed.
    fn apply(&mut self, name: &str, data: &Value) -> bool {
        let unreads = self.unreads.apply(name, data, &self.me, &self.my_roles);
        self.apply_servers(name, data) || unreads
    }

    fn apply_servers(&mut self, name: &str, data: &Value) -> bool {
        match name {
            "READY_SUPPLEMENTAL" => {
                for guild in data["guilds"].as_array().into_iter().flatten() {
                    let Some(id) = guild["id"].as_str() else {
                        continue;
                    };
                    for state in guild["voice_states"].as_array().into_iter().flatten() {
                        self.voice_state(id, state);
                    }
                }
                self.learn_members(&data["guilds"], &data["merged_members"]);
                true
            }
            "VOICE_STATE_UPDATE" => {
                let Some(guild) = data["guild_id"].as_str() else {
                    return false;
                };
                self.learn_member(guild, &data["member"]);
                self.voice_state(guild, data);
                true
            }
            "GUILD_CREATE" => {
                let Some(server) = ready::server(data) else {
                    return false;
                };
                for state in data["voice_states"].as_array().into_iter().flatten() {
                    self.voice_state(&server.id, state);
                }
                for member in data["members"].as_array().into_iter().flatten() {
                    self.learn_member(&server.id, member);
                }
                self.servers.retain(|known| known.id != server.id);
                self.servers.push(server);
                ready::sort_servers(&mut self.servers);
                true
            }
            "GUILD_MEMBER_UPDATE" => {
                let Some(guild) = data["guild_id"].as_str() else {
                    return false;
                };
                self.learn_member(guild, data);
                true
            }
            "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" | "GUILD_ROLE_DELETE" => {
                let Some(server) = data["guild_id"]
                    .as_str()
                    .and_then(|guild| self.servers.iter_mut().find(|server| server.id == guild))
                else {
                    return false;
                };
                if name == "GUILD_ROLE_DELETE" {
                    if let Some(role) = data["role_id"].as_str() {
                        server.remove_role(role);
                    }
                } else {
                    server.put_role(&data["role"]);
                }
                true
            }
            // Carries the server's own fields, without its channels or members.
            "GUILD_UPDATE" => {
                let (Some(update), Some(known)) = (
                    ready::server(data),
                    data["id"]
                        .as_str()
                        .and_then(|id| self.servers.iter_mut().find(|server| server.id == id)),
                ) else {
                    return false;
                };
                known.name = update.name;
                known.icon = update.icon;
                ready::sort_servers(&mut self.servers);
                true
            }
            "GUILD_DELETE" => {
                let Some(id) = data["id"].as_str() else {
                    return false;
                };
                self.servers.retain(|server| server.id != id);
                self.voice.retain(|(guild, _), _| guild != id);
                true
            }
            "CHANNEL_CREATE" | "CHANNEL_UPDATE" | "CHANNEL_DELETE" => {
                let (Some(guild), Some(id)) = (data["guild_id"].as_str(), data["id"].as_str())
                else {
                    return false;
                };
                let Some(server) = self.servers.iter_mut().find(|server| server.id == guild) else {
                    return false;
                };
                if name == "CHANNEL_DELETE" {
                    server.remove_channel(id);
                } else {
                    server.put_channel(data);
                }
                true
            }
            // Large servers report voice in these until subscribed.
            "PASSIVE_UPDATE_V1" | "PASSIVE_UPDATE_V2" => {
                let Some(guild) = data["guild_id"].as_str() else {
                    return false;
                };
                let members = data["members"]
                    .as_array()
                    .or_else(|| data["updated_members"].as_array());
                for member in members.into_iter().flatten() {
                    self.learn_member(guild, member);
                }
                if name == "PASSIVE_UPDATE_V1" {
                    self.voice.retain(|(server, _), _| server != guild);
                }
                let states = data["voice_states"]
                    .as_array()
                    .or_else(|| data["updated_voice_states"].as_array());
                for state in states.into_iter().flatten() {
                    self.voice_state(guild, state);
                }
                for user in data["removed_voice_states"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    if let Some(user) = user.as_str() {
                        self.voice.remove(&(guild.to_owned(), user.to_owned()));
                    }
                }
                true
            }
            "GUILD_MEMBERS_CHUNK" => {
                let Some(guild) = data["guild_id"].as_str() else {
                    return false;
                };
                for member in data["members"].as_array().into_iter().flatten() {
                    self.learn_member(guild, member);
                }
                true
            }
            _ => false,
        }
    }

    /// The voice channel this account is in, in `guild`.
    fn voice_channel(&self, guild: &str) -> Option<&str> {
        self.voice
            .get(&(guild.to_owned(), self.me.clone()))
            .map(|entry| entry.channel.as_str())
    }

    fn voice_session(&self, guild: &str) -> Option<&str> {
        self.voice
            .get(&(guild.to_owned(), self.me.clone()))
            .and_then(|entry| entry.session.as_deref())
    }

    fn voice_state(&mut self, guild: &str, state: &Value) {
        let Some(user) = state["user_id"].as_str() else {
            return;
        };
        let key = (guild.to_owned(), user.to_owned());
        match state["channel_id"].as_str() {
            Some(channel) => {
                let flag = |name: &str| state[name].as_bool().unwrap_or(false);
                self.voice.insert(
                    key,
                    VoiceEntry {
                        channel: channel.to_owned(),
                        session: state["session_id"].as_str().map(str::to_owned),
                        muted: flag("self_mute") || flag("mute"),
                        deafened: flag("self_deaf") || flag("deaf"),
                        streaming: flag("self_stream"),
                    },
                );
            }
            None => {
                self.voice.remove(&key);
            }
        }
    }

    fn learn_user(&mut self, user: &Value) {
        self.write_names().learn_user(user);
    }

    fn learn_member(&mut self, guild: &str, member: &Value) {
        self.write_names().learn_member(guild, member);
        let user = member["user_id"]
            .as_str()
            .or_else(|| member["user"]["id"].as_str());
        if user == Some(self.me.as_str()) {
            self.unreads.learn_own_member(guild, member);
        }
        if user == Some(self.me.as_str())
            && let Some(roles) = member["roles"].as_array()
        {
            let roles = roles
                .iter()
                .filter_map(|role| role.as_str().map(str::to_owned))
                .collect();
            self.my_roles.insert(guild.to_owned(), roles);
        }
    }

    /// `merged_members` lists members per guild, in the same order as `guilds`.
    fn learn_members(&mut self, guilds: &Value, merged: &Value) {
        let guilds = guilds.as_array().into_iter().flatten();
        let merged = merged.as_array().into_iter().flatten();
        for (guild, members) in guilds.zip(merged) {
            let Some(guild) = guild["id"].as_str() else {
                continue;
            };
            for member in members.as_array().into_iter().flatten() {
                self.learn_member(guild, member);
            }
        }
    }

    fn write_names(&self) -> std::sync::RwLockWriteGuard<'_, Names> {
        self.names
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn snapshot(&self) -> Snapshot {
        let names = self
            .names
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut voice: HashMap<String, Vec<VoiceMember>> = HashMap::new();
        for ((guild, user), entry) in &self.voice {
            voice
                .entry(entry.channel.clone())
                .or_default()
                .push(VoiceMember {
                    user_id: user.clone(),
                    name: names.name(guild, user).unwrap_or("Discord user").to_owned(),
                    avatar: names.avatar(Some(guild), user),
                    muted: entry.muted,
                    deafened: entry.deafened,
                    streaming: entry.streaming,
                });
        }
        for members in voice.values_mut() {
            members.sort_by(|a, b| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then_with(|| a.user_id.cmp(&b.user_id))
            });
        }
        // Mark what this account may do in each channel.
        let servers = self
            .servers
            .iter()
            .map(|server| {
                let roles = self.my_roles.get(&server.id).map_or(&[][..], Vec::as_slice);
                let mut marked = server.clone();
                for channel in marked
                    .text_channels
                    .iter_mut()
                    .chain(&mut marked.voice_channels)
                {
                    channel.access = server.access(channel, &self.me, roles);
                }
                self.unreads.mark(&mut marked);
                marked
            })
            .collect();
        Snapshot {
            status: Status::Online,
            user_id: Some(self.me.clone()),
            account: names.user_name(&self.me).map(|name| Account {
                name: name.to_owned(),
                avatar: names.avatar(None, &self.me),
            }),
            servers,
            voice,
            listed: self
                .list
                .as_ref()
                .and_then(|list| Some((list.channel().to_owned(), list.count()?))),
        }
    }
}

fn publish_members(tracker: &Tracker, shared: &Shared) {
    let Some(list) = &tracker.list else {
        return;
    };
    let server = tracker
        .servers
        .iter()
        .find(|server| server.id == list.guild());
    let names = shared
        .names
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = shared.live.send(Live::Members {
        channel: list.channel().to_owned(),
        members: list.members(server, &names),
    });
}

/// The live message events in one gateway event, learning the names it carries.
fn live_events(me: &str, names: &RwLock<Names>, name: &str, data: &Value) -> Vec<Live> {
    let channel = || data["channel_id"].as_str().unwrap_or_default().to_owned();
    let reaction = |added: bool| {
        let (key, emoji) = message::emoji(&data["emoji"])?;
        Some(Live::Reaction {
            channel: channel(),
            message: data["message_id"].as_str()?.to_owned(),
            key,
            emoji,
            me: data["user_id"] == me,
            added,
        })
    };
    match name {
        "MESSAGE_CREATE" | "MESSAGE_UPDATE" => {
            let Some(message) = message::parse(data) else {
                return vec![];
            };
            names
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .learn_message(data["guild_id"].as_str(), data);
            vec![if name == "MESSAGE_CREATE" {
                Live::Created(message)
            } else {
                Live::Updated(message)
            }]
        }
        "MESSAGE_DELETE" => data["id"]
            .as_str()
            .map(|id| Live::Deleted {
                channel: channel(),
                id: id.to_owned(),
            })
            .into_iter()
            .collect(),
        "MESSAGE_DELETE_BULK" => data["ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|id| Live::Deleted {
                channel: channel(),
                id: id.to_owned(),
            })
            .collect(),
        "MESSAGE_REACTION_ADD" => reaction(true).into_iter().collect(),
        "MESSAGE_REACTION_REMOVE" => reaction(false).into_iter().collect(),
        "MESSAGE_REACTION_REMOVE_ALL" | "MESSAGE_REACTION_REMOVE_EMOJI" => data["message_id"]
            .as_str()
            .map(|message| Live::ReactionsCleared {
                channel: channel(),
                message: message.to_owned(),
                key: message::emoji(&data["emoji"]).map(|(key, _)| key),
            })
            .into_iter()
            .collect(),
        // Answers to the member requests history makes.
        "GUILD_MEMBERS_CHUNK" => vec![Live::Names],
        "STREAM_CREATE" | "STREAM_UPDATE" => data["stream_key"]
            .as_str()
            .map(|key| Live::StreamUpdated {
                stream_key: key.to_owned(),
                viewers: data["viewer_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                paused: data["paused"].as_bool().unwrap_or(false),
            })
            .into_iter()
            .collect(),
        "STREAM_DELETE" => data["stream_key"]
            .as_str()
            .map(|key| Live::StreamEnded {
                stream_key: key.to_owned(),
                reason: data["reason"].as_str().map(str::to_owned),
            })
            .into_iter()
            .collect(),
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tracks_voice_channels_and_names() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "users": [{"id": "2", "username": "ann", "global_name": "Ann", "avatar": "abc"}],
            "guilds": [{"id": "10", "properties": {"name": "Test"}, "channels": [
                {"id": "11", "name": "Lounge", "type": 2, "position": 0},
            ]}],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        tracker.apply(
            "READY_SUPPLEMENTAL",
            &json!({
                "guilds": [{"id": "10", "voice_states": [
                    {"user_id": "2", "channel_id": "11", "self_mute": true},
                ]}],
                "merged_members": [[{"user_id": "2", "nick": "Annie"}]],
            }),
        );
        let snapshot = tracker.snapshot();
        let lounge = &snapshot.voice["11"];
        assert_eq!(lounge.len(), 1);
        assert_eq!(lounge[0].name, "Annie");
        assert!(lounge[0].avatar.ends_with("/avatars/2/abc.png?size=128"));
        assert!(lounge[0].muted);

        tracker.apply(
            "VOICE_STATE_UPDATE",
            &json!({"guild_id": "10", "user_id": "2", "channel_id": null}),
        );
        assert!(tracker.snapshot().voice.is_empty());

        tracker.apply(
            "CHANNEL_CREATE",
            &json!({"guild_id": "10", "id": "12", "name": "Games", "type": 2, "position": 1}),
        );
        let names: Vec<_> = tracker.servers[0]
            .voice_channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect();
        assert_eq!(names, ["Lounge", "Games"]);

        tracker.apply(
            "GUILD_UPDATE",
            &json!({"id": "10", "name": "Renamed", "icon": "ghi"}),
        );
        let server = &tracker.servers[0];
        assert_eq!(server.name, "Renamed");
        assert!(
            server
                .icon
                .as_ref()
                .is_some_and(|icon| icon.contains("/icons/10/ghi.png"))
        );
        assert_eq!(server.voice_channels.len(), 2, "the channels stay");
    }

    #[test]
    fn marks_voice_channels_this_account_can_join() {
        let hidden = r#"[{"id": "10", "type": 0, "allow": "0", "deny": "1024"},
                         {"id": "20", "type": 0, "allow": "1024", "deny": "0"}]"#;
        let ready = json!({
            "user": {"id": "1"},
            "guilds": [{"id": "10", "properties": {"name": "Test", "owner_id": "9"},
                "roles": [{"id": "10", "permissions": "1049600"}, {"id": "20", "permissions": "0"}],
                "channels": [
                    {"id": "11", "name": "Lounge", "type": 2, "position": 0},
                    {"id": "12", "name": "Staff", "type": 2, "position": 1,
                     "permission_overwrites": serde_json::from_str::<Value>(hidden).unwrap()},
                ]}],
            "merged_members": [[{"user_id": "1", "roles": []}]],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let channels = |tracker: &Tracker| -> Vec<String> {
            tracker.snapshot().servers[0]
                .voice_channels
                .iter()
                .filter(|channel| channel.access == ready::Access::Join)
                .map(|channel| channel.name.clone())
                .collect()
        };
        assert_eq!(channels(&tracker), ["Lounge"]);
        tracker.apply(
            "GUILD_MEMBER_UPDATE",
            &json!({"guild_id": "10", "user": {"id": "1"}, "roles": ["20"]}),
        );
        assert_eq!(channels(&tracker), ["Lounge", "Staff"]);
    }
}
