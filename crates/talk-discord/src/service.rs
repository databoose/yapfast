//! Talk's Discord connection: one gateway session kept open while Talk runs.
//! It tracks the account's servers, their channels and who is in voice,
//! passes on live message events, and hands the voice server details for a
//! join to the voice worker.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::{
    api::Api,
    cache,
    gateway::{Closed, Gateway, Invalidated},
    member_list::{self, ListedMember, MemberList, Presence},
    message::{self, Message, Names},
    profile::{self, Profile},
    ready::{self, Access, PRIVATE, Private, Server},
    settings::{self, CustomStatus},
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Account {
    pub name: String,
    /// The address of its avatar.
    pub avatar: String,
    /// Its Nitro: 0 none, 1 Nitro Classic, 2 Nitro, 3 Nitro Basic; see
    /// [`crate::api::upload_limit`].
    pub premium_type: u8,
}

/// A DM or group DM, as the DM list shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Conversation {
    pub id: String,
    pub name: String,
    /// The other person's avatar in a DM, or a group DM's icon.
    pub avatar: Option<String>,
    pub group: bool,
    /// Everyone in it, this account included.
    pub people: usize,
    /// A message request, waiting to be accepted or ignored. Requests count
    /// no mentions, and spam isn't unread either.
    pub request: bool,
    /// A request Discord thinks is spam.
    pub spam: bool,
    /// The other person in a DM; none in a group.
    pub person: Option<String>,
    pub unread: bool,
    /// Unread messages, as every message in a DM counts as a mention.
    pub mentions: u64,
    /// A call is going on here. Who is in it is in [`Snapshot::voice`].
    pub call: bool,
    /// Whom Discord is ringing for the call, this account perhaps among them.
    #[serde(skip)]
    pub ringing: Vec<VoiceMember>,
    /// A one-to-one DM's other person: whether they're around, when Discord
    /// says so of a friend, and their custom status or what they're doing.
    /// Not saved, since it's stale by the next start.
    #[serde(skip)]
    pub presence: Option<Presence>,
    #[serde(skip)]
    pub activity: Option<String>,
}

/// A friend request, sent to this account or by it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FriendRequest {
    pub user_id: String,
    pub name: String,
    pub avatar: String,
    /// Sent to this account, rather than by it.
    pub incoming: bool,
    /// Discord took it for spam.
    pub spam: bool,
}

/// A friend as the friends list shows them.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Friend {
    pub user_id: String,
    pub name: String,
    pub avatar: String,
    /// Whether they're around and their custom status or what they're doing,
    /// when Discord says. Not saved, since it's stale by the next start.
    #[serde(skip)]
    pub presence: Option<Presence>,
    #[serde(skip)]
    pub activity: Option<String>,
}

/// Someone this account can start a DM with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub avatar: String,
    /// A Discord friend, rather than someone known from a server or a message.
    pub friend: bool,
}

/// What the account looks like now. [`crate::cache`] keeps all but the live
/// parts, the connection and voice, to show on the next start.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    #[serde(skip)]
    pub status: Status,
    pub user_id: Option<String>,
    pub account: Option<Account>,
    /// Shared between snapshots while unchanged, so publishing one copies
    /// only the servers that changed.
    pub servers: Vec<Arc<Server>>,
    /// DMs and group DMs, the latest conversation first.
    pub conversations: Vec<Conversation>,
    /// The account's friends, by user id.
    pub friends: Vec<String>,
    /// The same friends by name, as the friends list shows them.
    #[serde(default)]
    pub friend_list: Vec<Friend>,
    /// Those this account blocked, by user id. Their messages show collapsed,
    /// and they don't notify.
    pub blocked: Vec<String>,
    /// This account's status: `online`, `idle`, `dnd` or `invisible`.
    pub presence: String,
    /// This account's custom status, until it ends.
    pub custom_status: Option<CustomStatus>,
    /// Friend requests waiting for an answer: those sent to this account
    /// first, then those it sent, the newest first in each.
    pub friend_requests: Vec<FriendRequest>,
    /// Who is in each voice channel, by channel id and then by name.
    #[serde(skip)]
    pub voice: HashMap<String, Vec<VoiceMember>>,
    /// The channel whose member list is on screen, and how many people can
    /// read it.
    #[serde(skip)]
    pub listed: Option<(String, u64)>,
}

/// A message event from the gateway, for any channel.
#[derive(Clone, Debug)]
pub enum Live {
    Created(Message),
    /// A new message that should notify this account, by its notification settings,
    /// in a server or, with none, a DM.
    Notify {
        guild: Option<String>,
        message: Message,
    },
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
    /// Someone else started typing; it lasts about ten seconds unless they type on.
    Typing {
        guild: Option<String>,
        channel: String,
        user: String,
        name: String,
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
    /// Discord moved this account's call in `guild`, or with [`PRIVATE`] its
    /// DM call, to another voice server, which the call connects to.
    VoiceMoved {
        guild: String,
        server: VoiceServer,
    },
    /// Discord moved a stream this account shares or watches to another
    /// server.
    StreamMoved(StreamServer),
}

/// What songbird needs to open the voice connection Discord assigned.
#[derive(Clone)]
pub struct VoiceServer {
    pub user_id: String,
    pub session_id: String,
    pub token: String,
    pub endpoint: String,
}

/// Leaves out the token, which no log should show.
impl std::fmt::Debug for VoiceServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceServer")
            .field("user_id", &self.user_id)
            .field("session_id", &self.session_id)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
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
    Search {
        guild: String,
        query: String,
        reply: oneshot::Sender<Vec<String>>,
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
    /// The custom status changed in the settings; this session shows it.
    CustomStatus(Option<CustomStatus>),
}

/// A handle to the connection. It ends when every handle is dropped.
#[derive(Clone)]
pub struct Discord {
    requests: mpsc::UnboundedSender<Request>,
    snapshots: watch::Receiver<Arc<Snapshot>>,
    live: broadcast::Sender<Live>,
    api: Arc<Api>,
    names: Arc<RwLock<Names>>,
    /// Profiles fetched lately, oldest first.
    profiles: Arc<Mutex<Vec<Kept>>>,
}

/// A profile fetched lately, for a user in a server.
struct Kept {
    user: String,
    guild: String,
    at: Instant,
    profile: Profile,
}

const LIVE_BACKLOG: usize = 256;
/// Profiles are fetched again after this long; Discord limits how often.
const PROFILE_AGE: Duration = Duration::from_secs(5 * 60);
const PROFILES: usize = 32;

impl Discord {
    /// Connects on the current Tokio runtime and keeps reconnecting until
    /// Discord rejects the token.
    pub fn start(token: String) -> Result<Self> {
        let api = Arc::new(Api::new(token.clone())?);
        let (requests, receiver) = mpsc::unbounded_channel();
        // The account as last seen shows while Talk connects.
        let cache::Loaded {
            snapshot: cached,
            names,
            lists,
        } = cache::load(&token);
        let presence = settings::presence(
            if cached.presence.is_empty() {
                "online"
            } else {
                &cached.presence
            },
            cached.custom_status.as_ref(),
            now_ms(),
        );
        let (snapshots, watcher) = watch::channel(Arc::new(cached));
        let (live, _) = broadcast::channel(LIVE_BACKLOG);
        let names = Arc::new(RwLock::new(names));
        tokio::spawn(cache::keep(watcher.clone(), names.clone()));
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
                lists,
                group: None,
                presence,
            },
        ));
        Ok(Self {
            requests,
            snapshots: watcher,
            live,
            api,
            names,
            profiles: Arc::default(),
        })
    }

    pub fn snapshots(&self) -> watch::Receiver<Arc<Snapshot>> {
        self.snapshots.clone()
    }

    /// Message events for every channel, from now on.
    pub fn live(&self) -> broadcast::Receiver<Live> {
        self.live.subscribe()
    }

    /// Tells Discord a server's channel is on screen, so its messages arrive
    /// live. With a text `channel`, its member list arrives as [`Live::Members`].
    /// DMs arrive live anyway; with [`PRIVATE`] as the server, a group DM's
    /// members arrive the same way.
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
        let server = server(guild);
        let mut unknown = HashSet::new();
        {
            let mut names = self.write_names();
            for value in &values {
                names.learn_message(server, value);
                if let (Some(guild), Some(author)) = (server, value["author"]["id"].as_str())
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
        files: Vec<(String, Vec<u8>)>,
    ) -> Result<Message> {
        let mut uploads = Vec::with_capacity(files.len());
        for (name, data) in files {
            match self.api.upload(channel, &name, data).await {
                Ok(upload) => uploads.push(upload),
                Err(error) => {
                    for upload in &uploads {
                        let _ = self.api.discard(upload).await;
                    }
                    return Err(error);
                }
            }
        }
        let sent = self
            .api
            .send(server(guild), channel, content, nonce, reply, &uploads)
            .await;
        if sent.is_err() {
            for upload in &uploads {
                let _ = self.api.discard(upload).await;
            }
        }
        let value = sent?;
        self.write_names().learn_message(server(guild), &value);
        message::parse(&value).ok_or_else(|| anyhow!("could not understand Discord's answer"))
    }

    pub async fn edit(&self, channel: &str, id: &str, content: &str) -> Result<Message> {
        let value = self.api.edit(channel, id, content).await?;
        message::parse(&value).ok_or_else(|| anyhow!("could not understand Discord's answer"))
    }

    pub async fn delete(&self, channel: &str, id: &str) -> Result<()> {
        self.api.delete(channel, id).await
    }

    /// A user's profile, in a server or with [`PRIVATE`] outside any. One
    /// fetched in the last few minutes is given again.
    pub async fn profile(&self, guild: &str, user: &str) -> Result<Profile> {
        let kept = |kept: &&Kept| kept.user == user && kept.guild == guild;
        if let Some(fresh) = self
            .lock_profiles()
            .iter()
            .filter(kept)
            .find(|fresh| fresh.at.elapsed() < PROFILE_AGE)
        {
            return Ok(fresh.profile.clone());
        }
        let value = self.api.profile(user, server(guild)).await?;
        self.write_names().learn_user(&value["user"]);
        let profile = profile::parse(&value, server(guild))
            .ok_or_else(|| anyhow!("could not understand Discord's answer"))?;
        let mut profiles = self.lock_profiles();
        profiles.retain(|old| !kept(&old));
        profiles.push(Kept {
            user: user.to_owned(),
            guild: guild.to_owned(),
            at: Instant::now(),
            profile: profile.clone(),
        });
        if profiles.len() > PROFILES {
            profiles.remove(0);
        }
        Ok(profile)
    }

    fn lock_profiles(&self) -> std::sync::MutexGuard<'_, Vec<Kept>> {
        self.profiles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Accepts a message request, or ignores it. The DM list changes when
    /// Discord confirms it.
    pub async fn answer_request(&self, channel: &str, accept: bool) -> Result<()> {
        if accept {
            self.api.accept_request(channel).await
        } else {
            self.api.ignore_request(channel).await
        }
    }

    /// Adds or removes this account's reaction; `emoji` is in the API's form.
    pub async fn react(&self, channel: &str, id: &str, emoji: &str, add: bool) -> Result<()> {
        self.api.react(channel, id, emoji, add).await
    }

    /// Shows others that this account is typing in `channel`, for about ten seconds.
    pub async fn typing(&self, channel: &str) -> Result<()> {
        self.api.typing(channel).await
    }

    /// Rings the others in a DM or group DM call this account started, as
    /// Discord's client does: in a DM, a friend at once, and anyone else only
    /// if Discord says they may be rung. Returns whether it rang.
    pub async fn ring(&self, channel: &str) -> Result<bool> {
        let stranger = {
            let snapshot = self.snapshots.borrow();
            snapshot
                .conversations
                .iter()
                .find(|conversation| conversation.id == channel)
                .and_then(|conversation| conversation.person.clone())
                .is_some_and(|person| !snapshot.friends.contains(&person))
        };
        if stranger && !self.api.ringable(channel).await? {
            return Ok(false);
        }
        self.api.ring(channel).await?;
        Ok(true)
    }

    /// Ends this login's session on Discord; the token stops working.
    pub async fn logout(&self) -> Result<()> {
        self.api.logout().await
    }

    /// Sends a friend request to whoever has this username.
    pub async fn add_friend_by_username(&self, username: &str) -> Result<()> {
        self.api.add_friend_by_username(username).await
    }

    /// Sets or clears the custom status, everywhere this account is signed
    /// in, and shows it from this session.
    pub async fn set_custom_status(&self, custom: Option<CustomStatus>) -> Result<()> {
        self.api.set_custom_status(custom.as_ref()).await?;
        let _ = self.requests.send(Request::CustomStatus(custom));
        Ok(())
    }

    /// Sends someone a friend request from their profile, or accepts theirs.
    /// The lists change when Discord confirms it.
    pub async fn befriend(&self, user: &str) -> Result<()> {
        let incoming = self
            .snapshots
            .borrow()
            .friend_requests
            .iter()
            .any(|request| request.user_id == user && request.incoming);
        // Discord's client accepts from its friends list and sends from a profile.
        let place = if incoming { "Friends" } else { "User Profile" };
        self.api.add_friend(user, place).await
    }

    /// Ignores a friend request to this account, or cancels one it sent.
    pub async fn remove_friend_request(&self, user: &str) -> Result<()> {
        self.api.remove_relationship(user, "Friends").await
    }

    /// Declines a call ringing this account; the call goes on without it.
    pub async fn decline(&self, channel: &str) -> Result<()> {
        let me = self.snapshots.borrow().user_id.clone().unwrap_or_default();
        self.api.stop_ringing(channel, &me).await
    }

    /// Uploads the preview Discord shows of this account's live stream: a
    /// JPEG as a data URI.
    pub async fn stream_preview(&self, stream_key: &str, thumbnail: &str) -> Result<()> {
        self.api.stream_preview(stream_key, thumbnail).await
    }

    /// Downloads a file from Discord's CDN, or a picture kept on disk.
    pub async fn download(&self, url: &str, limit: usize) -> Result<Vec<u8>> {
        if let Some(kept) = cache::picture(url).await.filter(|kept| kept.len() <= limit) {
            return Ok(kept);
        }
        let data = self.api.download(url, limit).await?;
        cache::keep_picture(url, data.clone()).await;
        Ok(data)
    }

    /// A user's name in a server: their nickname, display name or username.
    pub fn name(&self, guild: &str, user: &str) -> Option<String> {
        self.read_names().name(guild, user).map(str::to_owned)
    }

    /// The address of a user's avatar, in a server or in DMs.
    pub fn avatar(&self, guild: Option<&str>, user: &str) -> String {
        self.read_names().avatar(guild.and_then(server), user)
    }

    /// The ids of up to ten server members whose names start with `query`, whose
    /// names, avatars and roles are known once it answers.
    pub async fn search_members(&self, guild: &str, query: &str) -> Result<Vec<String>> {
        let (reply, found) = oneshot::channel();
        self.requests
            .send(Request::Search {
                guild: guild.into(),
                query: query.into(),
                reply,
            })
            .map_err(|_| anyhow!("Discord is offline."))?;
        tokio::time::timeout(SEARCH_TIMEOUT, found)
            .await
            .map_err(|_| anyhow!("Discord didn't answer the search."))?
            .map_err(|_| anyhow!("Discord disconnected during the search."))
    }

    /// Opens the DM with `user`, or finds the one already open; returns its channel
    /// id. The DM list has it once Discord's CHANNEL_CREATE arrives.
    pub async fn open_dm(&self, user: &str) -> Result<String> {
        self.api.open_dm(user).await
    }

    /// People to start a DM with whose names start with `query`, friends first; with
    /// an empty query, the friends alone.
    pub fn find_people(&self, query: &str) -> Vec<Person> {
        let snapshot = self.snapshots.borrow().clone();
        let me = snapshot.user_id.clone().unwrap_or_default();
        let names = self.read_names();
        let person = |id: &String, friend: bool| Person {
            id: id.clone(),
            name: names.user_name(id).unwrap_or("Discord user").to_owned(),
            avatar: names.avatar(None, id),
            friend,
        };
        let matches = |name: &str| {
            let (name, query) = (name.to_lowercase(), query.to_lowercase());
            name.starts_with(&query) || name.split_whitespace().any(|word| word.starts_with(&query))
        };
        let mut people: Vec<Person> = snapshot
            .friends
            .iter()
            .map(|id| person(id, true))
            .filter(|person| matches(&person.name))
            .collect();
        people.sort_by_key(|person| person.name.to_lowercase());
        if !query.is_empty() {
            for id in names.find_users(query) {
                if id != me && !snapshot.friends.contains(&id) {
                    people.push(person(&id, false));
                }
            }
        }
        people
    }

    /// The RGB colour of a member's name in a server: their highest coloured
    /// role's, once their roles are known.
    pub fn color(&self, server: &Server, user: &str) -> Option<u32> {
        server.color(self.read_names().roles(&server.id, user))
    }

    /// The users and roles a message mentions: `(text, role, id)`.
    pub fn mentions(&self, server: Option<&Server>, content: &str) -> Vec<(String, bool, String)> {
        self.read_names().mentions(server, content)
    }

    /// The members known to hold `role` in a server, by user id.
    pub fn members_with_role(&self, guild: &str, role: &str) -> Vec<String> {
        self.read_names().members_with_role(guild, role)
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

/// The server a channel is in, or none for DMs.
fn server(guild: &str) -> Option<&str> {
    (guild != PRIVATE).then_some(guild)
}

const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
/// The status that silences notifications, as [`Snapshot::presence`] gives it.
pub const DO_NOT_DISTURB: &str = "dnd";
/// Relationship types: a friend, someone blocked, and friend requests to and
/// from this account.
const FRIEND: u64 = 1;
const BLOCKED: u64 = 2;
const INCOMING_REQUEST: u64 = 3;
const OUTGOING_REQUEST: u64 = 4;
/// How long a member search waits for Discord.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(5);
/// Snapshots go out at most this often: the first change after a quiet
/// spell at once, and a burst of them together.
const PUBLISH_GAP: Duration = Duration::from_millis(100);
/// Reads within this long go to Discord as one acknowledgement per channel.
const ACK_DELAY: Duration = Duration::from_secs(1);
const FIRST_RETRY: Duration = Duration::from_secs(2);
const LAST_RETRY: Duration = Duration::from_secs(60);

struct Shared {
    snapshots: watch::Sender<Arc<Snapshot>>,
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
    /// The group DM on screen, whose members Talk lists itself.
    group: Option<String>,
    /// The presence this session last sent, which it identifies with again.
    presence: Value,
}

/// Member lists kept for channels shown again.
const LISTS: usize = 40;
/// How long a member list Discord sent stands without asking again. A list
/// Discord still sends stays current however old.
const LIST_FRESH: Duration = Duration::from_secs(10 * 60);

impl Shared {
    /// Keeps a member list Discord sent, for when a channel with the same
    /// readers is shown again.
    fn keep_list(&mut self, list: MemberList) {
        if !list.synced() {
            return;
        }
        self.lists
            .retain(|kept| kept.guild() != list.guild() || kept.readers() != list.readers());
        self.lists.push(list);
        if self.lists.len() > LISTS {
            self.lists.remove(0);
        }
        cache::keep_lists(self.lists.clone());
    }

    /// The member list kept for channels with these readers, shown for
    /// `channel`, or an empty one.
    fn take_list(&mut self, guild: &str, channel: &str, readers: &str) -> MemberList {
        match self
            .lists
            .iter()
            .position(|list| list.guild() == guild && list.readers() == readers)
        {
            Some(index) => {
                let mut list = self.lists.remove(index);
                list.show_for(channel);
                list
            }
            None => MemberList::new(guild.to_owned(), channel.to_owned(), readers.to_owned()),
        }
    }
}

/// Asks Discord for the member list on screen, unless Discord already sends
/// it or sent it within [`LIST_FRESH`]; then `stale` fires when it's due.
/// `subscribed` holds the readers each server's subscription lists.
async fn refresh_list(
    gateway: &mut Gateway,
    tracker: &mut Tracker,
    subscribed: &mut HashMap<String, String>,
    stale: std::pin::Pin<&mut tokio::time::Sleep>,
) -> Result<()> {
    let Some(list) = tracker.list.as_mut() else {
        return Ok(());
    };
    if subscribed
        .get(list.guild())
        .is_some_and(|readers| readers == list.readers())
    {
        return Ok(());
    }
    if list.heard_within(LIST_FRESH)
        && let Some(at) = list.stale_at(LIST_FRESH)
    {
        stale.reset(at.into());
        return Ok(());
    }
    // Discord's sync replaces what was kept, even under another id, as when
    // the readers changed meanwhile.
    list.revisit();
    subscribed.insert(list.guild().to_owned(), list.readers().to_owned());
    gateway.subscribe(list.guild(), Some(list.channel())).await
}

async fn run(token: String, mut requests: mpsc::UnboundedReceiver<Request>, mut shared: Shared) {
    let snapshots = shared.snapshots.clone();
    let mut retry = FIRST_RETRY;
    loop {
        snapshots.send_modify(|snapshot| Arc::make_mut(snapshot).status = Status::Connecting);
        let connected = tokio::select! {
            connected = Gateway::connect(&token, shared.presence.clone()) => connected,
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
                let snapshot = Arc::make_mut(snapshot);
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
            let snapshot = Arc::make_mut(snapshot);
            snapshot.status = Status::Offline("Can't reach Discord. Reconnecting...".into());
            snapshot.voice.clear();
        });
        // Discord asks for a random wait of one to five seconds after it ends
        // a session; other retries back off, spread so clients don't return
        // together.
        let wait = if error.downcast_ref::<Invalidated>().is_some() {
            Duration::from_millis(rand::random_range(1_000..5_000))
        } else {
            retry.mul_f64(rand::random_range(0.5..1.0))
        };
        let wait = tokio::time::sleep(wait);
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
        Request::View { guild, channel } if guild == PRIVATE => shared.group = channel,
        Request::View { guild, channel } => {
            if let Some(channel) = channel {
                show_kept_members(shared, &guild, &channel);
                shared.listing = Some((guild.clone(), channel));
            }
            shared.group = None;
            shared.viewing.insert(guild);
        }
        _ => {}
    }
}

/// Shows the member list kept for a channel while Discord can't send it,
/// as when starting from the saved account.
fn show_kept_members(shared: &Shared, guild: &str, channel: &str) {
    let snapshot = shared.snapshots.borrow().clone();
    let Some(server) = snapshot.servers.iter().find(|server| server.id == guild) else {
        return;
    };
    let readers = server.readers(channel);
    let Some(list) = shared
        .lists
        .iter()
        .find(|list| list.guild() == guild && list.readers() == readers && list.synced())
    else {
        return;
    };
    let members = {
        let names = shared
            .names
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        list.members(Some(server), &names)
    };
    let _ = shared.live.send(Live::Members {
        channel: channel.to_owned(),
        members,
    });
    if let Some(count) = list.count() {
        shared.snapshots.send_modify(|snapshot| {
            Arc::make_mut(snapshot).listed = Some((channel.to_owned(), count))
        });
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
    channel: String,
    reply: oneshot::Sender<Result<VoiceServer>>,
    session_id: Option<String>,
    server: Option<(String, String)>,
}

impl Pending {
    /// Whether a voice state or voice server event is about this join: in
    /// its server, or for a DM call, its channel, as those carry no server.
    fn answers(&self, data: &Value) -> bool {
        match server(&self.guild) {
            Some(guild) => data["guild_id"] == guild,
            None => data["guild_id"].is_null() && data["channel_id"] == self.channel.as_str(),
        }
    }
}

/// A stream started or watched, waiting for its server.
struct PendingStream {
    guild: String,
    reply: oneshot::Sender<Result<StreamServer>>,
    opening: Opening,
}

/// Unix milliseconds now.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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

/// Sends the gateway this session's presence when it changed.
async fn show_presence(
    gateway: &mut Gateway,
    tracker: &Tracker,
    shared: &mut Shared,
) -> Result<()> {
    let presence = tracker.own_presence();
    if presence != shared.presence {
        gateway.update_presence(presence.clone()).await?;
        shared.presence = presence;
    }
    Ok(())
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
    tracker.keep_unavailable(&shared.snapshots.borrow().servers);
    shared.snapshots.send_replace(Arc::new(tracker.snapshot()));
    // This session shows the status the settings hold, as Discord's own apps do.
    if let Err(error) = show_presence(&mut gateway, &tracker, shared).await {
        return Ended::Failed(error);
    }
    publish_group(&tracker, shared);
    tracker.list = shared.listing.clone().map(|(guild, channel)| {
        let readers = tracker.readers(&guild, &channel);
        shared.take_list(&guild, &channel, &readers)
    });
    // A new session has no subscriptions: those of servers on screen go again.
    let mut subscribed: HashMap<String, String> = HashMap::new();
    for guild in &shared.viewing {
        let mut channel = None;
        if let Some(list) = tracker.list.as_mut().filter(|list| list.guild() == guild) {
            list.revisit();
            subscribed.insert(guild.clone(), list.readers().to_owned());
            channel = Some(list.channel().to_owned());
        }
        if let Err(error) = gateway.subscribe(guild, channel.as_deref()).await {
            return Ended::Failed(error);
        }
    }
    let list_stale = tokio::time::sleep(LIST_FRESH);
    tokio::pin!(list_stale);
    let mut pending: Option<Pending> = None;
    let deadline = tokio::time::sleep(JOIN_TIMEOUT);
    tokio::pin!(deadline);
    let mut openings: HashMap<String, PendingStream> = HashMap::new();
    let stream_deadline = tokio::time::sleep(JOIN_TIMEOUT);
    tokio::pin!(stream_deadline);
    // The newest read message per channel, waiting to be acknowledged.
    let mut acks: HashMap<String, String> = HashMap::new();
    // Member searches waiting for their answer, by nonce.
    let mut searches: HashMap<String, oneshot::Sender<Vec<String>>> = HashMap::new();
    let mut search_nonce = 0u64;
    let ack_at = tokio::time::sleep(ACK_DELAY);
    tokio::pin!(ack_at);
    // When the custom status ends by itself.
    let status_ends = tokio::time::sleep(status_left(&tracker));
    tokio::pin!(status_ends);
    // The account changed since the last snapshot; one goes out when
    // `publish_at` comes, PUBLISH_GAP after the last.
    let mut stale = false;
    let mut scheduled = false;
    let mut published = tokio::time::Instant::now();
    let publish_at = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(publish_at);
    loop {
        if stale && !scheduled {
            publish_at
                .as_mut()
                .reset((published + PUBLISH_GAP).max(tokio::time::Instant::now()));
            scheduled = true;
        }
        tokio::select! {
            () = &mut publish_at, if scheduled => {
                shared.snapshots.send_replace(Arc::new(tracker.snapshot()));
                published = tokio::time::Instant::now();
                (stale, scheduled) = (false, false);
            }
            request = requests.recv() => {
                let Some(request) = request else {
                    gateway.close().await;
                    return Ended::Dropped;
                };
                let sent = match request {
                    Request::Join { guild, channel, reply } => {
                        let sent = gateway.update_voice_state(server(&guild), Some(&channel), true, false).await;
                        deadline.as_mut().reset(tokio::time::Instant::now() + JOIN_TIMEOUT);
                        if let Some(previous) = pending.replace(Pending { guild, channel, reply, session_id: None, server: None }) {
                            let _ = previous.reply.send(Err(anyhow!("Another voice join replaced this one.")));
                        }
                        sent
                    }
                    Request::Leave { guild } => {
                        if pending.as_ref().is_some_and(|pending| pending.guild == guild) {
                            pending = None;
                        }
                        gateway.update_voice_state(server(&guild), None, false, false).await
                    }
                    Request::SelfState { guild, channel, mute, deaf } => {
                        gateway.update_voice_state(server(&guild), Some(&channel), mute, deaf).await
                    }
                    Request::View { guild, channel } if guild == PRIVATE => {
                        shared.group = channel;
                        publish_group(&tracker, shared);
                        Ok(())
                    }
                    Request::View { guild, channel } => {
                        shared.group = None;
                        let new = shared.viewing.insert(guild.clone());
                        match channel {
                            Some(channel) => {
                                shared.listing = Some((guild.clone(), channel.clone()));
                                let readers = tracker.readers(&guild, &channel);
                                match tracker.list.as_mut() {
                                    // Channels with the same readers share a list.
                                    Some(list) if list.guild() == guild && list.readers() == readers => {
                                        list.show_for(&channel);
                                    }
                                    _ => {
                                        let list = shared.take_list(&guild, &channel, &readers);
                                        if let Some(previous) = tracker.list.replace(list) {
                                            shared.keep_list(previous);
                                        }
                                    }
                                }
                                // Shown at once from the cache, until Discord's sync.
                                if tracker.list.as_ref().is_some_and(MemberList::synced) {
                                    publish_members(&tracker, shared);
                                }
                                stale = true;
                                let sent = refresh_list(&mut gateway, &mut tracker, &mut subscribed, list_stale.as_mut()).await;
                                if new && !subscribed.contains_key(&guild) {
                                    gateway.subscribe(&guild, None).await.and(sent)
                                } else {
                                    sent
                                }
                            }
                            None if new => gateway.subscribe(&guild, None).await,
                            None => Ok(()),
                        }
                    }
                    Request::MarkRead { guild, channel, sync } => {
                        if let Some(message) = tracker.unreads.mark_read(&guild, &channel) {
                            stale = true;
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
                            let _ = reply.send(Err(anyhow!("Join the call before going live.")));
                            Ok(())
                        } else {
                            let key = stream::key(&guild, &channel, &me);
                            stream_deadline.as_mut().reset(tokio::time::Instant::now() + JOIN_TIMEOUT);
                            let waiting = PendingStream { guild: guild.clone(), reply, opening: Opening::default() };
                            if let Some(previous) = openings.insert(key.clone(), waiting) {
                                let _ = previous.reply.send(Err(anyhow!("Going live again replaced this attempt.")));
                            }
                            // Unpaused straight away, as Discord-video-stream
                            // does: viewers of a paused stream get no video.
                            match gateway.create_stream(server(&guild), &channel).await {
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
                                let _ = reply.send(Err(anyhow!("Join the call to watch its streams.")));
                                Ok(())
                            }
                            None => {
                                let _ = reply.send(Err(anyhow!("That isn't a stream Talk knows how to watch.")));
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
                    Request::CustomStatus(custom) => {
                        // The settings update that follows says the same; this
                        // shows it without waiting.
                        tracker.custom_status = custom;
                        stale = true;
                        status_ends.as_mut().reset(tokio::time::Instant::now() + status_left(&tracker));
                        show_presence(&mut gateway, &tracker, shared).await
                    }
                    Request::Search { guild, query, reply } => {
                        search_nonce += 1;
                        let nonce = format!("talk-search-{search_nonce}");
                        // Unanswered searches are dropped, so their callers stop waiting.
                        searches.retain(|_, reply| !reply.is_closed());
                        searches.insert(nonce.clone(), reply);
                        gateway.search_members(&guild, &query, 10, &nonce).await
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
            () = &mut status_ends, if tracker.custom_status.as_ref().is_some_and(|custom| custom.expires_at_ms.is_some()) => {
                tracker.custom_status = None;
                stale = true;
                if let Err(error) = show_presence(&mut gateway, &tracker, shared).await {
                    return Ended::Failed(error);
                }
            }
            () = &mut list_stale, if tracker.list.as_ref().is_some_and(|list| subscribed.get(list.guild()).is_none_or(|readers| readers != list.readers())) => {
                // The list on screen went stale without Discord sending it.
                if let Err(error) = refresh_list(&mut gateway, &mut tracker, &mut subscribed, list_stale.as_mut()).await {
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
                // A server update that no join or stream waits for moves one
                // under way.
                let awaited = match event.name.as_str() {
                    "VOICE_SERVER_UPDATE" => pending.as_ref().is_some_and(|waiting| waiting.answers(&event.data)),
                    "STREAM_SERVER_UPDATE" => event.data["stream_key"].as_str().is_some_and(|key| openings.contains_key(key)),
                    _ => true,
                };
                if let Some(waiting) = pending.as_mut() {
                    match event.name.as_str() {
                        "VOICE_STATE_UPDATE" if event.data["user_id"] == me.as_str() && waiting.answers(&event.data) => {
                            waiting.session_id = event.data["session_id"].as_str().map(str::to_owned);
                        }
                        // The endpoint is null while Discord is still allocating a server.
                        "VOICE_SERVER_UPDATE" if waiting.answers(&event.data) => {
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
                if event.name == "GUILD_MEMBER_LIST_UPDATE" {
                    let mut names = shared.names.write().unwrap_or_else(|poisoned| poisoned.into_inner());
                    let (mut changed, mut counted, mut synced) = (false, false, false);
                    if let Some(list) = tracker.list.as_mut() {
                        let (count, settled) = (list.count(), list.settled());
                        changed = list.apply(&event.data, &mut names);
                        counted = list.count() != count;
                        synced = !settled && list.settled();
                    }
                    // Or a list kept from before, which Discord still sends.
                    if !changed {
                        for kept in shared.lists.iter_mut().filter(|kept| kept.settled()) {
                            if kept.apply(&event.data, &mut names) {
                                break;
                            }
                        }
                    }
                    drop(names);
                    // A list Discord just sent is saved for the next start.
                    if synced {
                        cache::keep_lists(
                            shared.lists.iter().chain(tracker.list.as_ref()).cloned().collect(),
                        );
                    }
                    if changed {
                        publish_members(&tracker, shared);
                    }
                    if counted {
                        stale = true;
                    }
                }
                if event.name == "GUILD_MEMBERS_CHUNK"
                    && let Some(reply) = event.data["nonce"].as_str().and_then(|nonce| searches.remove(nonce))
                {
                    // Learn the members before answering, so their names are known.
                    tracker.apply(&event.name, &event.data);
                    let found = event.data["members"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|member| member["user"]["id"].as_str())
                        .map(str::to_owned)
                        .collect();
                    let _ = reply.send(found);
                }
                for live in live_events(&me, &shared.names, &event.name, &event.data) {
                    let _ = shared.live.send(live);
                }
                if event.name == "MESSAGE_CREATE"
                    && tracker.notifies(&event.data)
                    && let Some(message) = message::parse(&event.data)
                {
                    let guild = event.data["guild_id"].as_str().map(str::to_owned);
                    let _ = shared.live.send(Live::Notify { guild, message });
                }
                if tracker.apply(&event.name, &event.data) {
                    stale = true;
                }
                if !awaited && let Some(moved) = tracker.moved(&event.name, &event.data) {
                    let _ = shared.live.send(moved);
                }
                if event.name == "USER_SETTINGS_PROTO_UPDATE" {
                    status_ends.as_mut().reset(tokio::time::Instant::now() + status_left(&tracker));
                    if let Err(error) = show_presence(&mut gateway, &tracker, shared).await {
                        return Ended::Failed(error);
                    }
                }
                // Server events, such as presences in a server, don't change a group.
                if shared.group.is_some()
                    && event.data["guild_id"].is_null()
                    && matches!(
                        event.name.as_str(),
                        "READY_SUPPLEMENTAL" | "PRESENCE_UPDATE" | "CHANNEL_UPDATE"
                            | "CHANNEL_RECIPIENT_ADD" | "CHANNEL_RECIPIENT_REMOVE"
                    )
                {
                    publish_group(&tracker, shared);
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
    /// This account's Nitro, as [`Account::premium_type`].
    premium: u8,
    /// Changed through [`Self::server_mut`], which copies one a snapshot
    /// still shares.
    servers: Vec<Arc<Server>>,
    /// Each server as the last snapshot marked it, by id.
    published: HashMap<String, Published>,
    names: Arc<RwLock<Names>>,
    /// Where each user is in voice, by server and user id; DM and group DM
    /// calls go under [`PRIVATE`].
    voice: HashMap<(String, String), VoiceEntry>,
    /// The calls in DMs and group DMs, by channel id, with whom each rings.
    calls: HashMap<String, Vec<String>>,
    /// This account's roles, by server id.
    my_roles: HashMap<String, Vec<String>>,
    unreads: Unreads,
    /// The member list of the channel on screen.
    list: Option<MemberList>,
    /// DMs and group DMs.
    private: Vec<Private>,
    /// What friends are doing, for group DM member lists.
    presences: HashMap<String, (Presence, Option<String>)>,
    /// The account's friends, by user id.
    friends: Vec<String>,
    /// Those this account blocked, by user id.
    blocked: Vec<String>,
    /// The server list's order, from the settings.
    order: Vec<String>,
    /// This account's status and custom status, from its settings.
    status: String,
    custom_status: Option<CustomStatus>,
    /// Friend requests by user id: whether each came to this account, whether
    /// Discord took it for spam, and when it was sent.
    requests: HashMap<String, (bool, bool, String)>,
    /// The RTC server of each stream this account shares or watches, by key.
    streams: HashMap<String, String>,
}

/// A user's Nitro from their user object, as [`Account::premium_type`].
fn premium_type(user: &Value) -> u8 {
    user["premium_type"]
        .as_u64()
        .and_then(|premium| u8::try_from(premium).ok())
        .unwrap_or_default()
}

impl Tracker {
    fn from_ready(me: String, ready: &Value, names: Arc<RwLock<Names>>) -> Self {
        let mut tracker = Self {
            me,
            premium: premium_type(&ready["user"]),
            servers: ready::servers(ready).into_iter().map(Arc::new).collect(),
            published: HashMap::new(),
            names,
            voice: HashMap::new(),
            calls: HashMap::new(),
            my_roles: HashMap::new(),
            unreads: Unreads::from_ready(ready),
            list: None,
            private: Vec::new(),
            presences: HashMap::new(),
            friends: Vec::new(),
            blocked: Vec::new(),
            order: Vec::new(),
            status: "online".into(),
            custom_status: None,
            requests: HashMap::new(),
            streams: HashMap::new(),
        };
        tracker.sort_servers();
        if let Some(encoded) = ready["user_settings_proto"].as_str() {
            tracker.settings(encoded);
        }
        for relationship in ready["relationships"].as_array().into_iter().flatten() {
            tracker.relationship(relationship);
        }
        tracker.learn_user(&ready["user"]);
        for user in ready["users"].as_array().into_iter().flatten() {
            tracker.learn_user(user);
        }
        for channel in ready["private_channels"].as_array().into_iter().flatten() {
            tracker.put_private(channel);
        }
        tracker.learn_members(&ready["guilds"], &ready["merged_members"]);
        tracker
    }

    /// Keeps what `earlier` knew of servers READY gave as unavailable, as it
    /// gives only their ids.
    fn keep_unavailable(&mut self, earlier: &[Arc<Server>]) {
        for server in self.servers.iter_mut().filter(|server| server.unavailable) {
            if let Some(known) = earlier.iter().find(|known| known.id == server.id) {
                let mut known = Server::clone(known);
                known.unavailable = true;
                *server = Arc::new(known);
            }
        }
        self.sort_servers();
    }

    /// Applies one dispatched event; returns whether anything visible changed.
    fn apply(&mut self, name: &str, data: &Value) -> bool {
        let unreads = self.unreads.apply(name, data, &self.me, &self.my_roles);
        self.apply_servers(name, data) || unreads
    }

    fn apply_servers(&mut self, name: &str, data: &Value) -> bool {
        match name {
            "READY_SUPPLEMENTAL" => {
                // The DMs left out of READY to make it smaller.
                for channel in data["lazy_private_channels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    self.unreads.learn_channel(channel);
                    self.put_private(channel);
                }
                for presence in data["merged_presences"]["friends"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    self.presence(presence["user_id"].as_str(), presence);
                }
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
                match data["guild_id"].as_str() {
                    Some(guild) => {
                        self.learn_member(guild, &data["member"]);
                        self.voice_state(guild, data);
                    }
                    // In a DM or group DM call.
                    None => self.voice_state(PRIVATE, data),
                }
                true
            }
            // Calls in DMs and group DMs. Discord sends one for each call
            // going on when Talk connects, as its capabilities ask.
            "CALL_CREATE" | "CALL_UPDATE" => {
                let Some(channel) = data["channel_id"].as_str() else {
                    return false;
                };
                let ringing = data["ringing"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|user| user.as_str().map(str::to_owned))
                    .collect();
                self.calls.insert(channel.to_owned(), ringing);
                for state in data["voice_states"].as_array().into_iter().flatten() {
                    self.voice_state(PRIVATE, state);
                }
                true
            }
            "CALL_DELETE" => {
                let Some(channel) = data["channel_id"].as_str() else {
                    return false;
                };
                self.calls.remove(channel);
                self.voice
                    .retain(|(guild, _), entry| guild != PRIVATE || entry.channel != channel);
                true
            }
            "GUILD_CREATE" => {
                let Some(server) = ready::server(data) else {
                    return false;
                };
                // Still out: what was known of it stays.
                if server.unavailable
                    && let Some(known) = self.server_mut(&server.id)
                {
                    known.unavailable = true;
                    return true;
                }
                for state in data["voice_states"].as_array().into_iter().flatten() {
                    self.voice_state(&server.id, state);
                }
                for member in data["members"].as_array().into_iter().flatten() {
                    self.learn_member(&server.id, member);
                }
                self.servers.retain(|known| known.id != server.id);
                self.servers.push(Arc::new(server));
                self.sort_servers();
                true
            }
            "RELATIONSHIP_ADD" | "RELATIONSHIP_UPDATE" => {
                self.relationship(data);
                true
            }
            "RELATIONSHIP_REMOVE" => {
                let id = data["id"].as_str().or_else(|| data["user_id"].as_str());
                self.friends.retain(|friend| Some(friend.as_str()) != id);
                self.blocked.retain(|blocked| Some(blocked.as_str()) != id);
                if let Some(id) = id {
                    self.requests.remove(id);
                }
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
                    .and_then(|guild| self.server_mut(guild))
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
            // The settings of type 1 hold the status.
            "USER_SETTINGS_PROTO_UPDATE" if data["settings"]["type"] == 1 => {
                let Some(encoded) = data["settings"]["proto"].as_str() else {
                    return false;
                };
                self.settings(encoded);
                true
            }
            "GUILD_EMOJIS_UPDATE" => {
                let Some(server) = data["guild_id"]
                    .as_str()
                    .and_then(|guild| self.server_mut(guild))
                else {
                    return false;
                };
                server.emojis = ready::emojis(&data["emojis"]);
                true
            }
            // Carries the server's own fields, without its channels or members.
            "GUILD_UPDATE" => {
                let (Some(update), Some(known)) = (
                    ready::server(data),
                    data["id"].as_str().and_then(|id| self.server_mut(id)),
                ) else {
                    return false;
                };
                known.name = update.name;
                known.icon = update.icon;
                known.premium_tier = update.premium_tier;
                self.sort_servers();
                true
            }
            // This account's Nitro can change while connected.
            "USER_UPDATE" if data["id"].as_str() == Some(self.me.as_str()) => {
                let premium = premium_type(data);
                let changed = premium != self.premium;
                self.premium = premium;
                self.learn_user(data);
                changed
            }
            // An outage, with `unavailable`, keeps the server as last known
            // until its GUILD_CREATE; without, this account left it.
            "GUILD_DELETE" => {
                let Some(id) = data["id"].as_str() else {
                    return false;
                };
                if data["unavailable"].as_bool() == Some(true) {
                    if let Some(server) = self.server_mut(id) {
                        server.unavailable = true;
                    }
                } else {
                    self.servers.retain(|server| server.id != id);
                }
                self.voice.retain(|(guild, _), _| guild != id);
                true
            }
            "CHANNEL_CREATE" | "CHANNEL_UPDATE" | "CHANNEL_DELETE"
                if data["guild_id"].is_null() =>
            {
                let Some(id) = data["id"].as_str() else {
                    return false;
                };
                self.private.retain(|private| private.id != id);
                if name != "CHANNEL_DELETE" {
                    self.put_private(data);
                }
                true
            }
            "CHANNEL_RECIPIENT_ADD" | "CHANNEL_RECIPIENT_REMOVE" => {
                let (Some(channel), Some(user)) =
                    (data["channel_id"].as_str(), data["user"]["id"].as_str())
                else {
                    return false;
                };
                self.learn_user(&data["user"]);
                let Some(private) = self
                    .private
                    .iter_mut()
                    .find(|private| private.id == channel)
                else {
                    return false;
                };
                private.recipients.retain(|known| known != user);
                if name == "CHANNEL_RECIPIENT_ADD" {
                    private.recipients.push(user.to_owned());
                }
                true
            }
            // A DM moves to the top of the list with each new message.
            "MESSAGE_CREATE" if data["guild_id"].is_null() => {
                let channel = data["channel_id"].as_str().unwrap_or_default();
                self.private.iter().any(|private| private.id == channel)
            }
            // Friends' presences; those in servers go to the member list.
            "PRESENCE_UPDATE" if data["guild_id"].is_null() => {
                let user = data["user"]["id"].as_str();
                // The DM list shows the person of each one-to-one DM, and the
                // friends list each friend.
                self.presence(user, data)
                    && (self
                        .friends
                        .iter()
                        .any(|friend| Some(friend.as_str()) == user)
                        || self.private.iter().any(|private| {
                            !private.group && private.recipients.first().map(String::as_str) == user
                        }))
            }
            "CHANNEL_CREATE" | "CHANNEL_UPDATE" | "CHANNEL_DELETE" => {
                let (Some(guild), Some(id)) = (data["guild_id"].as_str(), data["id"].as_str())
                else {
                    return false;
                };
                let Some(server) = self.server_mut(guild) else {
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
            // Discord sends STREAM_CREATE to the streamer and those who
            // watch, naming the stream's server again when it moves.
            "STREAM_CREATE" | "STREAM_UPDATE" | "STREAM_DELETE" => {
                let Some(key) = data["stream_key"].as_str() else {
                    return false;
                };
                let rtc = data["rtc_server_id"].as_str().map(str::to_owned);
                match (name, rtc) {
                    ("STREAM_DELETE", _) => {
                        self.streams.remove(key);
                    }
                    ("STREAM_CREATE", Some(rtc)) => {
                        self.streams.insert(key.to_owned(), rtc);
                    }
                    ("STREAM_UPDATE", Some(rtc)) => {
                        if let Some(known) = self.streams.get_mut(key) {
                            *known = rtc;
                        }
                    }
                    _ => {}
                }
                false
            }
            _ => false,
        }
    }

    /// Where a call or stream this account is in went, from a server update
    /// that no join or stream waited for: Discord moved it to another server.
    fn moved(&self, name: &str, data: &Value) -> Option<Live> {
        // The endpoint is null while Discord is still allocating a server.
        let (token, endpoint) = (data["token"].as_str()?, data["endpoint"].as_str()?);
        match name {
            "VOICE_SERVER_UPDATE" => {
                let guild = data["guild_id"].as_str().unwrap_or(PRIVATE);
                let entry = self.voice.get(&(guild.to_owned(), self.me.clone()))?;
                // A DM call's update names its channel instead.
                if guild == PRIVATE && data["channel_id"] != entry.channel.as_str() {
                    return None;
                }
                Some(Live::VoiceMoved {
                    guild: guild.to_owned(),
                    server: VoiceServer {
                        user_id: self.me.clone(),
                        session_id: entry.session.clone()?,
                        token: token.to_owned(),
                        endpoint: endpoint.to_owned(),
                    },
                })
            }
            "STREAM_SERVER_UPDATE" => {
                let key = data["stream_key"].as_str()?;
                let (guild, _, _) = stream::parse(key)?;
                Some(Live::StreamMoved(StreamServer {
                    stream_key: key.to_owned(),
                    user_id: self.me.clone(),
                    session_id: self.voice_session(guild)?.to_owned(),
                    rtc_server_id: self.streams.get(key)?.clone(),
                    token: token.to_owned(),
                    endpoint: endpoint.to_owned(),
                }))
            }
            _ => None,
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

    /// Adds a DM or group DM from a channel object, learning the users it
    /// carries.
    fn put_private(&mut self, channel: &Value) {
        let Some(private) = ready::private_channel(channel) else {
            return;
        };
        for user in channel["recipients"].as_array().into_iter().flatten() {
            self.learn_user(user);
        }
        self.private.retain(|known| known.id != private.id);
        self.private.push(private);
    }

    /// Learns a friend's presence; returns whether it changed.
    fn presence(&mut self, user: Option<&str>, data: &Value) -> bool {
        let Some(user) = user else {
            return false;
        };
        let presence = (
            Presence::parse(&data["status"]),
            member_list::activity(&data["activities"]),
        );
        self.presences.insert(user.to_owned(), presence.clone()) != Some(presence)
    }

    /// A group DM's members by name, as Discord lists them.
    fn group_members(&self, channel: &str) -> Option<Vec<ListedMember>> {
        let group = self
            .private
            .iter()
            .find(|private| private.id == channel && private.group)?;
        let names = self
            .names
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let people = std::iter::once(&self.me).chain(&group.recipients);
        let heading = format!("Members — {}", group.recipients.len() + 1);
        let mut members: Vec<_> = people
            .map(|user| {
                let (presence, activity) = if *user == self.me {
                    (Presence::Online, None)
                } else {
                    self.presences.get(user).cloned().unwrap_or_default()
                };
                ListedMember {
                    user_id: user.clone(),
                    name: names.user_name(user).unwrap_or("Discord user").to_owned(),
                    avatar: names.avatar(None, user),
                    group: heading.clone(),
                    presence,
                    color: None,
                    activity,
                }
            })
            .collect();
        members.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.user_id.cmp(&b.user_id))
        });
        Some(members)
    }

    /// Whether a new message should notify this account. Message requests and
    /// spam don't, and nothing does while its status is Do Not Disturb.
    fn notifies(&self, message: &Value) -> bool {
        if self.status == DO_NOT_DISTURB {
            return false;
        }
        let channel = message["channel_id"].as_str().unwrap_or_default();
        let category = message["guild_id"].as_str().and_then(|guild| {
            let server = self.servers.iter().find(|server| server.id == guild)?;
            let channel = server
                .text_channels
                .iter()
                .find(|known| known.id == channel)?;
            channel.parent.as_deref()
        });
        let author = message["author"]["id"].as_str().unwrap_or_default();
        let unwanted = self.blocked.iter().any(|blocked| blocked == author)
            || self
                .private
                .iter()
                .any(|private| private.id == channel && (private.request || private.spam));
        !unwanted
            && self
                .unreads
                .notifies(message, &self.me, &self.my_roles, category)
    }

    /// DMs and group DMs, the latest conversation first.
    /// Learns the status from settings, all or some of them, in base64.
    fn settings(&mut self, encoded: &str) {
        let read = settings::read(encoded);
        if let Some(status) = read.status.filter(|status| !status.is_empty()) {
            self.status = status;
        }
        if let Some(custom) = read.custom {
            self.custom_status = custom;
        }
        if let Some(order) = settings::server_order(encoded) {
            self.order = order;
            self.sort_servers();
        }
    }

    /// Orders the servers as the account's server list does. Those its
    /// settings don't list yet go first, the latest joined first, as Discord
    /// shows a server just joined.
    /// A server to change, copied first if a snapshot shares it.
    fn server_mut(&mut self, id: &str) -> Option<&mut Server> {
        self.servers
            .iter_mut()
            .find(|server| server.id == id)
            .map(Arc::make_mut)
    }

    fn sort_servers(&mut self) {
        let place: HashMap<&str, usize> = self
            .order
            .iter()
            .enumerate()
            .map(|(index, id)| (id.as_str(), index))
            .collect();
        self.servers.sort_by(|a, b| {
            match (place.get(a.id.as_str()), place.get(b.id.as_str())) {
                (Some(a), Some(b)) => a.cmp(b),
                (Some(_), None) => std::cmp::Ordering::Greater,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (None, None) => b.joined_at.cmp(&a.joined_at),
            }
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
        });
    }

    /// Who can read a server's channel; see [`ready::Server::readers`].
    fn readers(&self, guild: &str, channel: &str) -> String {
        self.servers
            .iter()
            .find(|server| server.id == guild)
            .map_or_else(
                || format!("channel:{channel}"),
                |server| server.readers(channel),
            )
    }

    /// The presence this session shows, as the settings say.
    fn own_presence(&self) -> Value {
        settings::presence(&self.status, self.custom_status.as_ref(), now_ms())
    }

    /// Learns a relationship object: a friend, or someone who no longer is one.
    fn relationship(&mut self, relationship: &Value) {
        if relationship["user"].is_object() {
            self.learn_user(&relationship["user"]);
        }
        let Some(id) = relationship["id"]
            .as_str()
            .or_else(|| relationship["user_id"].as_str())
        else {
            return;
        };
        // An update may leave out the type, which then hasn't changed.
        let Some(kind) = relationship["type"].as_u64() else {
            return;
        };
        self.friends.retain(|friend| friend != id);
        self.blocked.retain(|blocked| blocked != id);
        self.requests.remove(id);
        match kind {
            FRIEND => self.friends.push(id.to_owned()),
            BLOCKED => self.blocked.push(id.to_owned()),
            INCOMING_REQUEST | OUTGOING_REQUEST => {
                let spam = relationship["is_spam_request"].as_bool().unwrap_or(false);
                let since = relationship["since"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                self.requests
                    .insert(id.to_owned(), (kind == INCOMING_REQUEST, spam, since));
            }
            _ => {}
        }
    }

    /// Friend requests waiting for an answer, incoming then outgoing, the
    /// newest first.
    fn friend_requests(&self, names: &Names) -> Vec<FriendRequest> {
        let mut requests: Vec<_> = self.requests.iter().collect();
        requests.sort_by(|(_, a), (_, b)| b.0.cmp(&a.0).then_with(|| b.2.cmp(&a.2)));
        requests
            .into_iter()
            .map(|(user, (incoming, spam, _))| FriendRequest {
                user_id: user.clone(),
                name: names.user_name(user).unwrap_or("Discord user").to_owned(),
                avatar: names.avatar(None, user),
                incoming: *incoming,
                spam: *spam,
            })
            .collect()
    }

    /// The friends by name, with what Discord says of each.
    fn friend_list(&self, names: &Names) -> Vec<Friend> {
        let mut friends: Vec<Friend> = self
            .friends
            .iter()
            .map(|user| {
                let known = self.presences.get(user);
                Friend {
                    user_id: user.clone(),
                    name: names.user_name(user).unwrap_or("Discord user").to_owned(),
                    avatar: names.avatar(None, user),
                    presence: known.map(|(presence, _)| *presence),
                    activity: known.and_then(|(_, activity)| activity.clone()),
                }
            })
            .collect();
        friends.sort_by_cached_key(|friend| (friend.name.to_lowercase(), friend.user_id.clone()));
        friends
    }

    fn conversations(&self, names: &Names) -> Vec<Conversation> {
        let mut private = self.private.clone();
        // Discord orders by the newest message, or when the DM opened.
        private.sort_by_cached_key(|private| {
            let opened = private.id.parse().unwrap_or_default();
            std::cmp::Reverse(self.unreads.last_message(&private.id).max(opened))
        });
        private
            .into_iter()
            .map(|mut private| {
                self.unreads.mark_private(&mut private);
                let name = |user: &String| names.user_name(user).unwrap_or("Discord user");
                let title = match (&private.name, private.recipients.as_slice()) {
                    (Some(name), _) => name.clone(),
                    (None, []) if private.group => "Unnamed group".into(),
                    (None, recipients) if private.group => {
                        recipients.iter().map(name).collect::<Vec<_>>().join(", ")
                    }
                    (None, [user, ..]) => name(user).to_owned(),
                    (None, []) => "Discord user".into(),
                };
                let avatar = if private.group {
                    private.icon.clone()
                } else {
                    private
                        .recipients
                        .first()
                        .map(|user| names.avatar(None, user))
                };
                let call = self.calls.contains_key(&private.id);
                let known = (!private.group)
                    .then(|| private.recipients.first())
                    .flatten()
                    .and_then(|user| self.presences.get(user));
                let ringing = self
                    .calls
                    .get(&private.id)
                    .into_iter()
                    .flatten()
                    .map(|user| VoiceMember {
                        user_id: user.clone(),
                        name: name(user).to_owned(),
                        avatar: names.avatar(None, user),
                        muted: false,
                        deafened: false,
                        streaming: false,
                    })
                    .collect();
                Conversation {
                    id: private.id,
                    name: title,
                    avatar,
                    group: private.group,
                    people: private.recipients.len() + 1,
                    request: private.request,
                    spam: private.spam,
                    person: (!private.group)
                        .then(|| private.recipients.first().cloned())
                        .flatten(),
                    unread: private.unread && !private.spam,
                    mentions: if private.request { 0 } else { private.mentions },
                    call,
                    ringing,
                    presence: known.map(|(presence, _)| *presence),
                    activity: known.and_then(|(_, activity)| activity.clone()),
                }
            })
            .collect()
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

    fn snapshot(&mut self) -> Snapshot {
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
                    avatar: names.avatar(server(guild), user),
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
        // Mark what this account may do in each channel, reusing the servers
        // the last snapshot marked the same.
        let mut published = HashMap::with_capacity(self.servers.len());
        let mut servers = Vec::with_capacity(self.servers.len());
        for server in &self.servers {
            let roles = self.my_roles.get(&server.id).map_or(&[][..], Vec::as_slice);
            let last = self
                .published
                .remove(&server.id)
                .filter(|last| Arc::ptr_eq(&last.source, server) && last.roles == roles);
            // What the account may do stays while the server and its roles do.
            let marks = match &last {
                Some(last) => last.marks.unread(server, &self.unreads),
                None => Marks::new(server, &self.me, roles, &self.unreads),
            };
            let (marked, roles) = match last {
                Some(last) if last.marks == marks => (last.marked, last.roles),
                Some(last) => (Arc::new(marks.apply(server)), last.roles),
                None => (Arc::new(marks.apply(server)), roles.to_vec()),
            };
            servers.push(marked.clone());
            published.insert(
                server.id.clone(),
                Published {
                    source: server.clone(),
                    roles,
                    marks,
                    marked,
                },
            );
        }
        self.published = published;
        Snapshot {
            status: Status::Online,
            user_id: Some(self.me.clone()),
            account: names.user_name(&self.me).map(|name| Account {
                name: name.to_owned(),
                avatar: names.avatar(None, &self.me),
                premium_type: self.premium,
            }),
            servers,
            conversations: self.conversations(&names),
            friends: self.friends.clone(),
            friend_list: self.friend_list(&names),
            blocked: self.blocked.clone(),
            presence: self.status.clone(),
            custom_status: self
                .custom_status
                .clone()
                .filter(|custom| !custom.expired(now_ms())),
            friend_requests: self.friend_requests(&names),
            voice,
            listed: self
                .list
                .as_ref()
                .and_then(|list| Some((list.channel().to_owned(), list.count()?))),
        }
    }
}

/// A server as a snapshot published it: marked from `source` with `marks`,
/// for an account with `roles` there.
struct Published {
    source: Arc<Server>,
    roles: Vec<String>,
    marks: Marks,
    marked: Arc<Server>,
}

/// What [`Tracker::snapshot`] sets on a server for this account: whether
/// it's muted, then for each text channel and each voice channel, in order,
/// what [`ChannelMarks`] holds.
#[derive(PartialEq, Eq)]
struct Marks {
    muted: bool,
    channels: Vec<ChannelMarks>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ChannelMarks {
    access: Access,
    mention_everyone: bool,
    muted: bool,
    unread: bool,
    mentions: u64,
}

impl Marks {
    fn new(server: &Server, me: &str, roles: &[String], unreads: &Unreads) -> Self {
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
            });
        }
        for channel in &server.voice_channels {
            channels.push(ChannelMarks {
                access: server.access(channel, me, roles),
                mention_everyone: server.may_mention_everyone(channel, me, roles),
                muted: channel.muted,
                unread: channel.unread,
                mentions: channel.mentions,
            });
        }
        Self {
            muted: unreads.server_muted(&server.id),
            channels,
        }
    }

    /// These marks with the unread state of `server`'s text channels as
    /// it is now.
    fn unread(&self, server: &Server, unreads: &Unreads) -> Self {
        let mut channels = self.channels.clone();
        for (channel, marks) in server.text_channels.iter().zip(&mut channels) {
            (marks.muted, marks.unread, marks.mentions) =
                unreads.channel(&server.id, channel, marks.access);
        }
        Self {
            muted: unreads.server_muted(&server.id),
            channels,
        }
    }

    /// A copy of `server` with these marks.
    fn apply(&self, server: &Server) -> Server {
        let mut marked = server.clone();
        marked.muted = self.muted;
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
        }
        marked
    }
}

/// The members of the group DM on screen.
fn publish_group(tracker: &Tracker, shared: &Shared) {
    let Some(channel) = &shared.group else {
        return;
    };
    if let Some(members) = tracker.group_members(channel) {
        let _ = shared.live.send(Live::Members {
            channel: channel.clone(),
            members,
        });
    }
}

fn publish_members(tracker: &Tracker, shared: &Shared) {
    let Some(list) = &tracker.list else {
        return;
    };
    let server = tracker
        .servers
        .iter()
        .find(|server| server.id == list.guild())
        .map(Arc::as_ref);
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
        "TYPING_START" => {
            let Some(user) = data["user_id"].as_str().filter(|user| *user != me) else {
                return vec![];
            };
            let guild = data["guild_id"].as_str();
            let mut names = names
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(guild) = guild
                && data["member"].is_object()
            {
                names.learn_member(guild, &data["member"]);
            }
            let name = guild
                .and_then(|guild| names.name(guild, user))
                .or_else(|| names.user_name(user))
                .unwrap_or("Someone")
                .to_owned();
            vec![Live::Typing {
                guild: guild.map(str::to_owned),
                channel: channel(),
                user: user.to_owned(),
                name,
            }]
        }
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
    fn keeps_servers_through_an_outage() {
        let guild = |id: &str, name: &str| {
            json!({"id": id, "properties": {"name": name}, "channels": [
                {"id": format!("{id}1"), "name": "general", "type": 0, "position": 0},
            ]})
        };
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "guilds": [guild("10", "Test"), guild("20", "Other")],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let listed = |tracker: &mut Tracker| -> Vec<(String, bool, usize)> {
            let mut servers: Vec<_> = tracker
                .snapshot()
                .servers
                .iter()
                .map(|server| {
                    let channels = server.text_channels.len();
                    (server.name.clone(), server.unavailable, channels)
                })
                .collect();
            servers.sort();
            servers
        };

        tracker.apply("GUILD_DELETE", &json!({"id": "10", "unavailable": true}));
        assert_eq!(
            listed(&mut tracker),
            [("Other".into(), false, 1), ("Test".into(), true, 1)],
            "an outage keeps the server as last known"
        );
        // READY in the outage gives only its id; what was shown stays.
        let earlier = tracker.snapshot().servers;
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "guilds": [{"id": "10", "unavailable": true}, guild("20", "Other")],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        tracker.keep_unavailable(&earlier);
        assert_eq!(
            listed(&mut tracker),
            [("Other".into(), false, 1), ("Test".into(), true, 1)]
        );

        tracker.apply("GUILD_CREATE", &guild("10", "Test"));
        assert_eq!(
            listed(&mut tracker),
            [("Other".into(), false, 1), ("Test".into(), false, 1)],
            "it's back with its GUILD_CREATE"
        );

        // Without `unavailable`, this account left the server.
        tracker.apply("GUILD_DELETE", &json!({"id": "10"}));
        assert_eq!(listed(&mut tracker), [("Other".into(), false, 1)]);
    }

    #[test]
    fn tracks_dm_calls_and_whom_they_ring() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "users": [{"id": "2", "username": "ann", "global_name": "Ann", "avatar": "abc"}],
            "private_channels": [{"id": "20", "type": 1, "recipient_ids": ["2"]}],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        tracker.apply(
            "CALL_CREATE",
            &json!({"channel_id": "20", "message_id": "30", "region": "rotterdam",
            "ringing": ["1"], "voice_states": [
                {"user_id": "2", "channel_id": "20", "session_id": "s", "self_mute": true},
            ]}),
        );
        let snapshot = tracker.snapshot();
        let call = &snapshot.voice["20"];
        assert_eq!((call[0].name.as_str(), call[0].muted), ("Ann", true));
        assert!(call[0].avatar.ends_with("/avatars/2/abc.png?size=128"));
        let ringing: Vec<_> = snapshot.conversations[0]
            .ringing
            .iter()
            .map(|person| (person.user_id.as_str(), person.name.as_str()))
            .collect();
        assert_eq!(ringing, [("1", "me")]);

        // Answering: this account joins, without a server, and stops ringing.
        tracker.apply(
            "VOICE_STATE_UPDATE",
            &json!({"user_id": "1", "channel_id": "20", "session_id": "mine"}),
        );
        tracker.apply(
            "CALL_UPDATE",
            &json!({"channel_id": "20", "message_id": "30", "ringing": []}),
        );
        assert_eq!(tracker.voice_channel(PRIVATE), Some("20"));
        assert_eq!(tracker.voice_session(PRIVATE), Some("mine"));
        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.voice["20"].len(), 2);
        assert!(snapshot.conversations[0].ringing.is_empty());

        tracker.apply("CALL_DELETE", &json!({"channel_id": "20"}));
        assert!(tracker.snapshot().voice.is_empty());
    }

    #[test]
    fn dms_show_their_persons_presence() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "users": [
                {"id": "2", "username": "ann", "global_name": "Ann"},
                {"id": "3", "username": "cy"},
            ],
            "private_channels": [
                {"id": "20", "type": 1, "recipient_ids": ["2"]},
                {"id": "30", "type": 3, "recipient_ids": ["2", "3"], "name": "Group"},
            ],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        assert_eq!(
            tracker.snapshot().conversations[0].presence,
            None,
            "unknown until Discord says"
        );
        let away = json!({"user": {"id": "2"}, "status": "idle",
            "activities": [{"type": 4, "name": "Custom Status", "state": "making tea"}]});
        assert!(tracker.apply("PRESENCE_UPDATE", &away), "Ann's DM shows it");
        assert!(
            !tracker.apply("PRESENCE_UPDATE", &away),
            "the same again changes nothing"
        );
        let snapshot = tracker.snapshot();
        let dm = snapshot
            .conversations
            .iter()
            .find(|dm| dm.id == "20")
            .unwrap();
        assert_eq!(dm.presence, Some(Presence::Idle));
        assert_eq!(dm.activity.as_deref(), Some("making tea"));
        let group = snapshot
            .conversations
            .iter()
            .find(|dm| dm.id == "30")
            .unwrap();
        assert_eq!(group.presence, None, "a group has no one person");
        // Someone with no one-to-one DM doesn't change the list.
        assert!(!tracker.apply(
            "PRESENCE_UPDATE",
            &json!({"user": {"id": "3"}, "status": "online"})
        ));
    }

    #[test]
    fn orders_servers_as_the_settings_do() {
        let guild = |id: &str, name: &str, joined: &str| json!({"id": id, "name": name, "joined_at": joined, "channels": [], "roles": []});
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "guilds": [
                guild("10", "Alpha", "2026-01-01T00:00:00+00:00"),
                guild("20", "Bravo", "2026-02-01T00:00:00+00:00"),
                guild("30", "Charlie", "2026-03-01T00:00:00+00:00"),
                guild("40", "Delta", "2026-04-01T00:00:00+00:00"),
            ],
            // Charlie on its own, then a folder of Alpha and Delta; Bravo is
            // missing, as a server joined from another client may be.
            "user_settings_proto": settings::write_server_order(&[&[30], &[10, 40]]),
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let order = |tracker: &Tracker| -> Vec<String> {
            tracker
                .servers
                .iter()
                .map(|server| server.name.clone())
                .collect()
        };
        assert_eq!(order(&tracker), ["Bravo", "Charlie", "Alpha", "Delta"]);

        // A server just joined goes first, until the settings place it.
        tracker.apply(
            "GUILD_CREATE",
            &guild("50", "Echo", "2026-05-01T00:00:00+00:00"),
        );
        assert_eq!(
            order(&tracker),
            ["Echo", "Bravo", "Charlie", "Alpha", "Delta"]
        );
        tracker.apply(
            "USER_SETTINGS_PROTO_UPDATE",
            &json!({"settings": {"type": 1, "proto":
                settings::write_server_order(&[&[40], &[50], &[30], &[20], &[10]])}}),
        );
        assert_eq!(
            order(&tracker),
            ["Delta", "Echo", "Charlie", "Bravo", "Alpha"]
        );

        // Changing only the status leaves the order alone.
        tracker.apply(
            "USER_SETTINGS_PROTO_UPDATE",
            &json!({"settings": {"type": 1, "proto": settings::write_custom(None)}, "partial": true}),
        );
        assert_eq!(
            order(&tracker),
            ["Delta", "Echo", "Charlie", "Bravo", "Alpha"]
        );
    }

    #[test]
    fn shows_the_status_the_settings_hold() {
        let away = CustomStatus {
            text: "Fixing the bridge".into(),
            emoji_name: Some("🔧".into()),
            ..Default::default()
        };
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "user_settings_proto": settings::write_custom(Some(&away)),
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        assert_eq!(tracker.snapshot().custom_status, Some(away));
        assert_eq!(tracker.snapshot().presence, "online");
        assert_eq!(
            tracker.own_presence()["activities"][0]["state"],
            "Fixing the bridge"
        );

        // Settings of another type say nothing of it; clearing it does.
        let cleared = settings::write_custom(None);
        assert!(!tracker.apply(
            "USER_SETTINGS_PROTO_UPDATE",
            &json!({"settings": {"type": 2, "proto": cleared}, "partial": true}),
        ));
        assert!(tracker.snapshot().custom_status.is_some());
        assert!(tracker.apply(
            "USER_SETTINGS_PROTO_UPDATE",
            &json!({"settings": {"type": 1, "proto": cleared}, "partial": true}),
        ));
        assert_eq!(tracker.snapshot().custom_status, None);
        assert_eq!(tracker.own_presence()["activities"], json!([]));
    }

    #[test]
    fn tracks_friend_requests_both_ways() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "relationships": [
                {"id": "2", "type": 1, "user": {"id": "2", "username": "ann"}},
                {"id": "3", "type": 3, "since": "2026-10-01T10:00:00+00:00",
                    "user": {"id": "3", "username": "bob"}},
                {"id": "4", "type": 4, "since": "2026-10-02T10:00:00+00:00",
                    "user": {"id": "4", "username": "cat"}},
                {"id": "5", "type": 3, "since": "2026-10-03T10:00:00+00:00",
                    "is_spam_request": true, "user": {"id": "5", "username": "spam"}},
            ],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let listed = |tracker: &mut Tracker| -> Vec<(String, bool, bool)> {
            tracker
                .snapshot()
                .friend_requests
                .into_iter()
                .map(|request| (request.name, request.incoming, request.spam))
                .collect()
        };
        assert_eq!(
            listed(&mut tracker),
            [
                ("spam".to_owned(), true, true),
                ("bob".to_owned(), true, false),
                ("cat".to_owned(), false, false),
            ],
            "those sent to this account first, the newest first"
        );

        // Accepting Bob's makes him a friend; Cat's is cancelled.
        tracker.apply(
            "RELATIONSHIP_ADD",
            &json!({"id": "3", "type": 1, "user": {"id": "3", "username": "bob"}}),
        );
        tracker.apply("RELATIONSHIP_REMOVE", &json!({"id": "4", "type": 4}));
        tracker.apply("RELATIONSHIP_UPDATE", &json!({"id": "5", "nickname": "x"}));
        assert_eq!(listed(&mut tracker), [("spam".to_owned(), true, true)]);
        assert_eq!(tracker.snapshot().friends, ["2", "3"]);
        let friends: Vec<_> = tracker
            .snapshot()
            .friend_list
            .into_iter()
            .map(|friend| (friend.name, friend.presence))
            .collect();
        assert_eq!(
            friends,
            [("ann".to_owned(), None), ("bob".to_owned(), None)],
            "by name"
        );
        // A friend's presence changes the friends list, though there's no DM with them.
        assert!(tracker.apply(
            "PRESENCE_UPDATE",
            &json!({"user": {"id": "3"}, "status": "idle", "activities": []}),
        ));
        assert_eq!(
            tracker.snapshot().friend_list[1].presence,
            Some(Presence::Idle)
        );

        // Blocking Ann ends the friendship, and her messages stop notifying.
        tracker.apply("RELATIONSHIP_ADD", &json!({"id": "2", "type": 2}));
        assert_eq!(tracker.snapshot().friends, ["3"]);
        assert_eq!(tracker.snapshot().blocked, ["2"]);
        let from =
            |author: &str| json!({"channel_id": "9", "author": {"id": author}, "content": "hi"});
        assert!(!tracker.notifies(&from("2")));
        tracker.apply("RELATIONSHIP_REMOVE", &json!({"id": "2", "type": 2}));
        assert!(tracker.snapshot().blocked.is_empty());
        assert!(tracker.notifies(&from("2")));

        // Do Not Disturb silences everyone, until the status changes.
        tracker.status = DO_NOT_DISTURB.into();
        assert!(!tracker.notifies(&from("3")));
        tracker.status = "idle".into();
        assert!(tracker.notifies(&from("3")));
    }

    #[test]
    fn follows_calls_and_streams_to_new_servers() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "guilds": [{"id": "10", "properties": {"name": "Test"}, "channels": [
                {"id": "11", "name": "Lounge", "type": 2, "position": 0},
            ]}],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let update =
            json!({"guild_id": "10", "token": "new", "endpoint": "moved.discord.media:443"});
        assert!(
            tracker.moved("VOICE_SERVER_UPDATE", &update).is_none(),
            "not in a call there"
        );
        tracker.apply(
            "VOICE_STATE_UPDATE",
            &json!({"guild_id": "10", "user_id": "1", "channel_id": "11", "session_id": "s"}),
        );
        let Some(Live::VoiceMoved { guild, server }) =
            tracker.moved("VOICE_SERVER_UPDATE", &update)
        else {
            panic!("the call moved");
        };
        assert_eq!(guild, "10");
        assert_eq!(
            (
                server.session_id.as_str(),
                server.token.as_str(),
                server.endpoint.as_str()
            ),
            ("s", "new", "moved.discord.media:443")
        );
        let allocating = json!({"guild_id": "10", "token": "new", "endpoint": null});
        assert!(tracker.moved("VOICE_SERVER_UPDATE", &allocating).is_none());

        // A stream moves where its last STREAM_CREATE said.
        let key = stream::key("10", "11", "1");
        let server_update =
            json!({"stream_key": key, "token": "t2", "endpoint": "moved.discord.media:443"});
        assert!(
            tracker
                .moved("STREAM_SERVER_UPDATE", &server_update)
                .is_none(),
            "a stream it isn't in"
        );
        tracker.apply(
            "STREAM_CREATE",
            &json!({"stream_key": key, "rtc_server_id": "rtc"}),
        );
        tracker.apply(
            "STREAM_CREATE",
            &json!({"stream_key": key, "rtc_server_id": "rtc2"}),
        );
        let Some(Live::StreamMoved(server)) = tracker.moved("STREAM_SERVER_UPDATE", &server_update)
        else {
            panic!("the stream moved");
        };
        assert_eq!(
            (
                server.rtc_server_id.as_str(),
                server.session_id.as_str(),
                server.token.as_str()
            ),
            ("rtc2", "s", "t2")
        );
        tracker.apply(
            "STREAM_DELETE",
            &json!({"stream_key": key, "reason": "user_requested"}),
        );
        assert!(
            tracker
                .moved("STREAM_SERVER_UPDATE", &server_update)
                .is_none()
        );
    }

    #[test]
    fn answers_a_dm_join_by_its_channel() {
        let (reply, _) = oneshot::channel();
        let waiting = Pending {
            guild: PRIVATE.into(),
            channel: "20".into(),
            reply,
            session_id: None,
            server: None,
        };
        assert!(waiting.answers(&json!({"guild_id": null, "channel_id": "20"})));
        assert!(!waiting.answers(&json!({"guild_id": null, "channel_id": "21"})));
        assert!(!waiting.answers(&json!({"guild_id": "10", "channel_id": "20"})));
    }

    #[test]
    fn lists_dms_newest_first_with_unreads() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "users": [
                {"id": "2", "username": "ann", "global_name": "Ann", "avatar": "abc"},
                {"id": "3", "username": "bob"},
            ],
            "private_channels": [
                {"id": "20", "type": 1, "recipient_ids": ["2"], "last_message_id": "500"},
                {"id": "21", "type": 3, "recipient_ids": ["2", "3"], "last_message_id": "600"},
                {"id": "22", "type": 3, "name": "Plans", "icon": "def", "recipient_ids": ["3"]},
                {"id": "23", "type": 1, "recipient_ids": ["3"], "is_message_request": true},
            ],
            "read_state": {"entries": [{"id": "21", "last_message_id": "550", "mention_count": 1}]},
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let listed = |tracker: &mut Tracker| -> Vec<(String, bool, u64)> {
            tracker
                .snapshot()
                .conversations
                .into_iter()
                .filter(|dm| !dm.request)
                .map(|dm| (dm.name, dm.unread, dm.mentions))
                .collect()
        };
        assert_eq!(
            listed(&mut tracker),
            [
                ("Ann, bob".into(), true, 1),
                ("Ann".into(), false, 0),
                ("Plans".into(), false, 0),
            ],
            "a DM without a read state is read"
        );
        let snapshot = tracker.snapshot();
        let dm = |id: &str| {
            snapshot
                .conversations
                .iter()
                .find(|dm| dm.id == id)
                .unwrap()
        };
        let ann = dm("20");
        assert!(!ann.group && ann.people == 2);
        assert!(
            ann.avatar
                .as_deref()
                .unwrap()
                .ends_with("/avatars/2/abc.png?size=128")
        );
        assert!(
            dm("22")
                .avatar
                .as_deref()
                .unwrap()
                .contains("/channel-icons/22/def.png")
        );
        assert_eq!(dm("21").avatar, None);

        // Ann writes: her DM comes first, and each message counts.
        let message = |id: &str| json!({"id": id, "channel_id": "20", "author": {"id": "2"}});
        assert!(tracker.apply("MESSAGE_CREATE", &message("700")));
        tracker.apply("MESSAGE_CREATE", &message("701"));
        assert_eq!(listed(&mut tracker)[0], ("Ann".into(), true, 2));
        assert_eq!(
            tracker.unreads.mark_read(PRIVATE, "20").as_deref(),
            Some("701")
        );
        assert_eq!(listed(&mut tracker)[0], ("Ann".into(), false, 0));

        // A muted DM shows nothing.
        tracker.apply(
            "USER_GUILD_SETTINGS_UPDATE",
            &json!({"guild_id": null, "channel_overrides": [{"channel_id": "20", "muted": true}]}),
        );
        tracker.apply("MESSAGE_CREATE", &message("702"));
        assert_eq!(listed(&mut tracker)[0], ("Ann".into(), false, 0));

        // DMs open, close and change their people.
        tracker.apply(
            "CHANNEL_CREATE",
            &json!({"id": "800", "type": 1, "recipients": [{"id": "4", "username": "cat"}]}),
        );
        assert_eq!(listed(&mut tracker)[0].0, "cat");
        tracker.apply("CHANNEL_DELETE", &json!({"id": "800", "type": 1}));
        tracker.apply(
            "CHANNEL_RECIPIENT_ADD",
            &json!({"channel_id": "22", "user": {"id": "4", "username": "cat"}}),
        );
        tracker.apply(
            "CHANNEL_RECIPIENT_REMOVE",
            &json!({"channel_id": "22", "user": {"id": "3"}}),
        );
        tracker.apply(
            "PRESENCE_UPDATE",
            &json!({"user": {"id": "4"}, "status": "idle", "activities": []}),
        );
        let members: Vec<_> = tracker
            .group_members("22")
            .unwrap()
            .into_iter()
            .map(|member| (member.name, member.group, member.presence))
            .collect();
        assert_eq!(
            members,
            [
                ("cat".into(), "Members — 2".into(), Presence::Idle),
                ("me".into(), "Members — 2".into(), Presence::Online),
            ]
        );
        assert!(
            tracker.group_members("20").is_none(),
            "a DM lists no members"
        );
        assert_eq!(
            tracker.snapshot().conversations.len(),
            4,
            "with the request"
        );
    }

    #[test]
    fn message_requests_wait_to_be_answered() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "users": [{"id": "2", "username": "ann"}, {"id": "3", "username": "spam"}],
            "private_channels": [
                {"id": "20", "type": 1, "recipient_ids": ["2"], "is_message_request": true},
                {"id": "21", "type": 1, "recipient_ids": ["3"], "is_message_request": true, "is_spam": true},
            ],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        for (id, channel, author) in [("500", "20", "2"), ("501", "21", "3")] {
            tracker.apply(
                "MESSAGE_CREATE",
                &json!({"id": id, "channel_id": channel, "author": {"id": author}}),
            );
        }
        let listed: Vec<_> = tracker
            .snapshot()
            .conversations
            .into_iter()
            .map(|dm| (dm.name, dm.request, dm.spam, dm.unread, dm.mentions))
            .collect();
        assert_eq!(
            listed,
            [
                ("spam".into(), true, true, false, 0),
                ("ann".into(), true, false, true, 0)
            ],
            "requests count no mentions, and spam isn't unread"
        );

        // Accepting makes it a DM; ignoring removes it.
        tracker.apply(
            "CHANNEL_UPDATE",
            &json!({"id": "20", "type": 1, "recipient_ids": ["2"], "is_message_request": false}),
        );
        tracker.apply("CHANNEL_DELETE", &json!({"id": "21", "type": 1}));
        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.conversations.len(), 1);
        assert!(!snapshot.conversations[0].request);
        assert_eq!(snapshot.conversations[0].mentions, 1);
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
        let channels = |tracker: &mut Tracker| -> Vec<String> {
            tracker.snapshot().servers[0]
                .voice_channels
                .iter()
                .filter(|channel| channel.access == ready::Access::Join)
                .map(|channel| channel.name.clone())
                .collect()
        };
        assert_eq!(channels(&mut tracker), ["Lounge"]);
        tracker.apply(
            "GUILD_MEMBER_UPDATE",
            &json!({"guild_id": "10", "user": {"id": "1"}, "roles": ["20"]}),
        );
        assert_eq!(channels(&mut tracker), ["Lounge", "Staff"]);
    }

    #[test]
    fn snapshots_share_the_servers_that_stay_the_same() {
        let ready = json!({
            "user": {"id": "1"},
            "guilds": [
                {"id": "10", "properties": {"name": "One"}, "joined_at": "2026-01-02T00:00:00+00:00",
                 "channels": [{"id": "11", "name": "general", "type": 0, "position": 0}]},
                {"id": "20", "properties": {"name": "Two"}, "joined_at": "2026-01-01T00:00:00+00:00",
                 "channels": [{"id": "21", "name": "general", "type": 0, "position": 0}]},
            ],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready, Default::default());
        let first = tracker.snapshot();
        let same = tracker.snapshot();
        assert!(Arc::ptr_eq(&first.servers[0], &same.servers[0]));
        assert!(Arc::ptr_eq(&first.servers[1], &same.servers[1]));

        // A message marks one server's channel unread; the other stays shared.
        tracker.apply(
            "MESSAGE_CREATE",
            &json!({"id": "900", "channel_id": "21", "guild_id": "20", "author": {"id": "2"}}),
        );
        let unread = tracker.snapshot();
        assert!(Arc::ptr_eq(&first.servers[0], &unread.servers[0]));
        assert!(!Arc::ptr_eq(&first.servers[1], &unread.servers[1]));
        assert!(unread.servers[1].text_channels[0].unread);
        assert!(
            !first.servers[1].text_channels[0].unread,
            "published ones don't change"
        );

        // Changing a server copies it rather than the one a snapshot holds.
        tracker.apply("GUILD_UPDATE", &json!({"id": "10", "name": "Renamed"}));
        let renamed = tracker.snapshot();
        assert_eq!(renamed.servers[0].name, "Renamed");
        assert_eq!(unread.servers[0].name, "One");
        assert!(Arc::ptr_eq(&unread.servers[1], &renamed.servers[1]));
    }
}
