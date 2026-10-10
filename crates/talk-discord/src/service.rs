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
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::{
    api::Api,
    cache,
    member_list::{ListedMember, Presence},
    message::{self, Message, Names},
    profile::{self, Profile},
    ready::{PRIVATE, Server},
    settings::{self, CustomStatus},
    stream::StreamServer,
};

mod live;
mod marks;
mod member_lists;
mod member_search;
mod publisher;
mod read_acks;
mod session;
mod stream_openings;
mod tracker;
mod voice_join;

use member_lists::MemberLists;
use session::Shared;

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
    /// The snowflake of the last message this account read here, or 0.
    #[serde(default)]
    pub last_read: u64,
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
    /// The folders in the account's server list. Their servers sit together
    /// in [`Self::servers`], in the folder's order.
    #[serde(default)]
    pub folders: Vec<settings::Folder>,
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
    /// The status changed in the settings; this session shows it.
    Status(String),
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
        tokio::spawn(session::run(
            token,
            receiver,
            Shared {
                snapshots,
                live: live.clone(),
                names: names.clone(),
                api: api.clone(),
                lists: MemberLists::new(lists, live.clone(), names.clone()),
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

    /// Sets the status, `online`, `idle`, `dnd` or `invisible`, everywhere
    /// this account is signed in, and shows it from this session.
    pub async fn set_status(&self, status: &str) -> Result<()> {
        self.api.set_status(status).await?;
        let _ = self.requests.send(Request::Status(status.to_owned()));
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

    /// Downloads a file from Discord's CDN into `file` as it arrives,
    /// returning its size; a video plays while it does.
    pub async fn download_into(
        &self,
        url: &str,
        file: &mut std::fs::File,
        limit: u64,
    ) -> Result<u64> {
        self.api.download_into(url, file, limit).await
    }

    /// Changes whenever a name, avatar or role that messages show changes.
    pub fn names_version(&self) -> u64 {
        self.read_names().version()
    }

    /// A user's name in a server: their nickname, display name or username.
    pub fn name(&self, guild: &str, user: &str) -> Option<String> {
        self.read_names().name(guild, user).map(str::to_owned)
    }

    /// A user's username, such as `ann`.
    pub fn username(&self, user: &str) -> Option<String> {
        self.read_names().username(user).map(str::to_owned)
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

/// The status that silences notifications, as [`Snapshot::presence`] gives it.
pub const DO_NOT_DISTURB: &str = "dnd";
/// How long a member search waits for Discord.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Unix milliseconds now.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
