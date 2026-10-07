//! Discord servers, channels and DMs in Talk's lists. They appear as servers,
//! channels and DMs with ids that can't collide with Matrix ones, so the
//! rail, channel list, DM list, timeline, voice dock and the one-call rule
//! cover them. A DM's id takes `@me` for its server, as in Discord's links.
//! The ids are known in every build; listing needs the `discord` feature.
#[cfg(feature = "discord")]
use std::sync::Arc;

#[cfg(feature = "discord")]
use talk_discord::{
    ready::{Access, PRIVATE},
    service::{Discord, Snapshot},
};
#[cfg(feature = "discord")]
use tokio::{
    sync::{Semaphore, mpsc::UnboundedSender},
    task::JoinSet,
};

#[cfg(feature = "discord")]
use crate::chat::{self, ChatRoom, ChatServer, Event, VoicePerson};

const PREFIX: &str = "discord:";
/// The categories message requests are listed under in the DM list.
pub const REQUESTS: &str = "Message requests";
pub const SPAM: &str = "Spam";
/// Icons and avatars are small; anything larger isn't one.
#[cfg(feature = "discord")]
const MAX_PICTURE_BYTES: usize = 4 * 1024 * 1024;
/// Profile banners, at the full profile's width; Discord's are 5:2.
#[cfg(feature = "discord")]
const BANNER_SIZE: (u32, u32) = (600, 240);
/// The size Discord serves them at; a larger one is shrunk to it.
#[cfg(feature = "discord")]
const PICTURE_SIZE: u32 = 128;
#[cfg(feature = "discord")]
const PARALLEL_DOWNLOADS: usize = 4;

/// The server part of a DM's room id.
pub const DM_GUILD: &str = "@me";

pub fn server_id(guild: &str) -> String {
    format!("{PREFIX}{guild}")
}

pub fn room_id(guild: &str, channel: &str) -> String {
    format!("{PREFIX}{guild}:{channel}")
}

pub fn is_discord(id: &str) -> bool {
    id.starts_with(PREFIX)
}

/// The Discord server and channel of a channel's room id.
pub fn parse_room(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(PREFIX)?.split_once(':')
}

#[cfg(feature = "discord")]
/// The servers, channels and DMs to list, in Discord's order: by category,
/// text channels before voice channels in each, and the latest DM first.
/// They stay listed while Discord reconnects; joining and sending wait for
/// the connection. Voice channels this account can see but not join are
/// listed locked, as in Discord, and text channels it can't send in are
/// read-only. Channels hidden from it are listed locked only with
/// `show_hidden`.
pub fn index(snapshot: &Snapshot, show_hidden: bool) -> (Vec<ChatRoom>, Vec<ChatServer>) {
    let premium = snapshot
        .account
        .as_ref()
        .map_or(0, |account| account.premium_type);
    let mut rooms: Vec<_> = snapshot
        .conversations
        .iter()
        .map(|dm| ChatRoom {
            id: room_id(PRIVATE, &dm.id),
            name: dm.name.clone(),
            direct: true,
            voice: false,
            encrypted: false,
            members: dm.people as u64,
            unread: u64::from(dm.unread),
            mentions: dm.mentions,
            locked: false,
            // Replying waits until the request is accepted.
            read_only: dm.request,
            category: match (dm.request, dm.spam) {
                (_, true) => SPAM,
                (true, false) => REQUESTS,
                _ => "",
            }
            .into(),
            request: dm.request,
            person: dm.person.clone(),
            people: vec![],
            avatar: dm.avatar.clone(),
            upload_limit: talk_discord::api::upload_limit(premium, None),
        })
        .collect();
    let mut servers = Vec::new();
    for server in &snapshot.servers {
        let text = server.text_channels.iter().map(|channel| (false, channel));
        let voice = server.voice_channels.iter().map(|channel| (true, channel));
        let mut listed: Vec<_> = text
            .chain(voice)
            .filter(|(_, channel)| show_hidden || channel.access != Access::Hidden)
            .collect();
        listed.sort_by_cached_key(|(voice, channel)| {
            (
                server.category_order(channel),
                *voice,
                channel.position,
                channel.id.clone(),
            )
        });
        let mut channels = Vec::new();
        for (voice, channel) in listed {
            let id = room_id(&server.id, &channel.id);
            let category = server
                .category(channel)
                .map(|category| category.name.clone())
                .unwrap_or_default();
            rooms.push(if voice {
                ChatRoom {
                    id: id.clone(),
                    name: channel.name.clone(),
                    direct: false,
                    voice: true,
                    // Discord requires DAVE end-to-end encryption in voice calls.
                    encrypted: true,
                    members: snapshot
                        .voice
                        .get(&channel.id)
                        .map_or(0, |members| members.len() as u64),
                    unread: 0,
                    mentions: 0,
                    locked: channel.access != Access::Join,
                    read_only: false,
                    category,
                    request: false,
                    person: None,
                    people: snapshot
                        .voice
                        .get(&channel.id)
                        .into_iter()
                        .flatten()
                        .map(|member| VoicePerson {
                            id: member.user_id.clone(),
                            name: member.name.clone(),
                            avatar: Some(member.avatar.clone()),
                            muted: member.muted,
                            deafened: member.deafened,
                            streaming: member.streaming,
                        })
                        .collect(),
                    avatar: None,
                    upload_limit: talk_discord::api::upload_limit(
                        premium,
                        Some(server.premium_tier),
                    ),
                }
            } else {
                ChatRoom {
                    id: id.clone(),
                    name: channel.name.clone(),
                    direct: false,
                    voice: false,
                    // Discord messages aren't end-to-end encrypted.
                    encrypted: false,
                    // Known for the channel whose member list is on screen.
                    members: snapshot
                        .listed
                        .as_ref()
                        .filter(|(listed, _)| *listed == channel.id)
                        .map_or(0, |(_, count)| *count),
                    unread: u64::from(channel.unread),
                    mentions: channel.mentions,
                    locked: channel.access == Access::Hidden,
                    read_only: channel.access == Access::Locked,
                    category,
                    request: false,
                    person: None,
                    people: vec![],
                    avatar: None,
                    upload_limit: talk_discord::api::upload_limit(
                        premium,
                        Some(server.premium_tier),
                    ),
                }
            });
            channels.push(id);
        }
        servers.push(ChatServer {
            id: server_id(&server.id),
            name: server.name.clone(),
            channels,
            discord: true,
            muted: server.muted,
            icon: server.icon.clone(),
        });
    }
    (rooms, servers)
}

/// Downloads server icons and avatars, and reports each as [`Event::Picture`].
/// Dropping it cancels the downloads.
#[cfg(feature = "discord")]
pub(crate) struct Pictures {
    slots: Arc<Semaphore>,
    tasks: JoinSet<()>,
}

#[cfg(feature = "discord")]
impl Pictures {
    pub(crate) fn new() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(PARALLEL_DOWNLOADS)),
            tasks: JoinSet::new(),
        }
    }

    /// Starts downloading `urls`, a few at a time. A picture that fails is
    /// logged and left out; its initial shows instead.
    pub(crate) fn fetch(
        &mut self,
        discord: &Discord,
        urls: Vec<String>,
        events: &UnboundedSender<Event>,
    ) {
        while self.tasks.try_join_next().is_some() {}
        for url in urls {
            let (discord, events, slots) = (discord.clone(), events.clone(), self.slots.clone());
            self.tasks.spawn(async move {
                let Ok(_slot) = slots.acquire().await else {
                    return;
                };
                let data = match discord.download(&url, MAX_PICTURE_BYTES).await {
                    Ok(data) => data,
                    Err(error) => {
                        tracing::debug!(%error, %url, "could not download a Discord picture");
                        return;
                    }
                };
                let (width, height) = if url.contains("/banners/") {
                    BANNER_SIZE
                } else {
                    (PICTURE_SIZE, PICTURE_SIZE)
                };
                let decoded =
                    tokio::task::spawn_blocking(move || chat::decode_picture(&data, width, height))
                        .await;
                match decoded {
                    Ok(Some(picture)) => {
                        let _ = events.send(Event::Picture {
                            url,
                            picture: Arc::new(picture),
                        });
                    }
                    _ => tracing::debug!(%url, "could not decode a Discord picture"),
                }
            });
        }
    }
}

#[cfg(all(test, feature = "discord"))]
mod tests {
    use talk_discord::ready::{Channel, Server};

    use super::*;

    #[test]
    fn lists_channels_by_category() {
        let channel = |id: &str, parent: Option<&str>, position| Channel {
            id: id.into(),
            name: id.into(),
            parent: parent.map(Into::into),
            position,
            ..Default::default()
        };
        let server = Server {
            id: "10".into(),
            text_channels: vec![
                channel("21", None, 5),
                channel("22", Some("2"), 0),
                channel("23", Some("1"), 0),
            ],
            voice_channels: vec![channel("31", Some("1"), 0), channel("32", None, 0)],
            categories: [("1", "Talk", 1), ("2", "Games", 2)]
                .map(|(id, name, position)| {
                    let category = talk_discord::ready::Category {
                        name: name.into(),
                        position,
                    };
                    (id.to_owned(), category)
                })
                .into(),
            ..Default::default()
        };
        let snapshot = Snapshot {
            servers: vec![server],
            ..Default::default()
        };
        let (rooms, _) = index(&snapshot, false);
        let listed: Vec<_> = rooms
            .iter()
            .map(|room| (room.name.as_str(), room.category.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                ("21", ""),
                ("32", ""),
                ("23", "Talk"),
                ("31", "Talk"),
                ("22", "Games")
            ]
        );
    }

    #[test]
    fn lists_channels_under_their_server() {
        let channel = |id: &str, access| Channel {
            id: id.into(),
            name: id.into(),
            access,
            ..Default::default()
        };
        let snapshot = Snapshot {
            servers: vec![Server {
                id: "10".into(),
                name: "Test".into(),
                text_channels: vec![
                    channel("21", Access::Join),
                    channel("22", Access::Locked),
                    channel("23", Access::Hidden),
                ],
                voice_channels: vec![
                    channel("11", Access::Join),
                    channel("12", Access::Locked),
                    channel("13", Access::Hidden),
                ],
                icon: Some("icon.png".into()),
                ..Default::default()
            }],
            voice: [(
                "11".to_owned(),
                vec![talk_discord::service::VoiceMember {
                    user_id: "2".into(),
                    name: "Ann".into(),
                    avatar: "ann.png".into(),
                    muted: true,
                    deafened: false,
                    streaming: true,
                }],
            )]
            .into(),
            ..Default::default()
        };
        let snapshot = Snapshot {
            listed: Some(("21".into(), 4)),
            ..snapshot
        };
        let (rooms, servers) = index(&snapshot, false);
        assert_eq!(
            (rooms[0].members, rooms[1].members),
            (4, 0),
            "a listed text channel counts its readers"
        );
        assert_eq!(rooms[0].id, "discord:10:21");
        assert_eq!(
            rooms[2].people[0].name, "Ann",
            "who is in voice shows without joining"
        );
        assert!(rooms[2].people[0].muted && rooms[3].people.is_empty());
        assert!(rooms[2].people[0].streaming, "who is live shows");
        assert_eq!(rooms[2].people[0].avatar.as_deref(), Some("ann.png"));
        assert!(rooms.iter().all(|room| room.category.is_empty()));
        assert!(!rooms[0].voice && !rooms[0].encrypted && !rooms[0].read_only);
        assert!(rooms[1].read_only && !rooms[1].locked);
        assert_eq!(rooms[2].id, "discord:10:11");
        assert!(rooms[2].voice && !rooms[2].locked);
        assert!(rooms[3].locked);
        assert_eq!(rooms.len(), 4, "hidden channels stay hidden");
        assert_eq!(servers[0].id, "discord:10");
        assert_eq!(servers[0].icon.as_deref(), Some("icon.png"));
        assert_eq!(
            servers[0].channels,
            [
                "discord:10:21",
                "discord:10:22",
                "discord:10:11",
                "discord:10:12"
            ]
        );
        let (rooms, _) = index(&snapshot, true);
        assert!(rooms.len() == 6 && rooms[2].locked && rooms[5].locked);
        assert_eq!(parse_room("discord:10:11"), Some(("10", "11")));
        assert_eq!(parse_room("!room:example.org"), None);
    }
}
