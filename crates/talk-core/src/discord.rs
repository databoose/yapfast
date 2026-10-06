//! Discord servers and channels in Talk's lists. They appear as servers and
//! channels with ids that can't collide with Matrix ones, so the rail, channel
//! list, timeline, voice dock and the one-call rule cover them.
use talk_discord::{ready::Access, service::Snapshot};

use crate::chat::{ChatRoom, ChatServer, VoicePerson};

const PREFIX: &str = "discord:";

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

/// The servers and channels to list, in Discord's order: by category, text
/// channels before voice channels in each. They stay listed while Discord
/// reconnects; joining and sending wait for the connection. Voice channels
/// this account can see but not join are listed locked, as in Discord, and
/// text channels it can't send in are read-only. Channels hidden from it are
/// listed locked only with `show_hidden`.
pub fn index(snapshot: &Snapshot, show_hidden: bool) -> (Vec<ChatRoom>, Vec<ChatServer>) {
    let mut rooms = Vec::new();
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
                    people: snapshot
                        .voice
                        .get(&channel.id)
                        .into_iter()
                        .flatten()
                        .map(|member| VoicePerson {
                            id: member.user_id.clone(),
                            name: member.name.clone(),
                            muted: member.muted,
                            deafened: member.deafened,
                        })
                        .collect(),
                }
            } else {
                ChatRoom {
                    id: id.clone(),
                    name: channel.name.clone(),
                    direct: false,
                    voice: false,
                    // Discord messages aren't end-to-end encrypted.
                    encrypted: false,
                    members: 0,
                    unread: u64::from(channel.unread),
                    mentions: channel.mentions,
                    locked: channel.access == Access::Hidden,
                    read_only: channel.access == Access::Locked,
                    category,
                    people: vec![],
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
        });
    }
    (rooms, servers)
}

#[cfg(test)]
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
                ..Default::default()
            }],
            voice: [(
                "11".to_owned(),
                vec![talk_discord::service::VoiceMember {
                    user_id: "2".into(),
                    name: "Ann".into(),
                    muted: true,
                    deafened: false,
                }],
            )]
            .into(),
            ..Default::default()
        };
        let (rooms, servers) = index(&snapshot, false);
        assert_eq!(rooms[0].id, "discord:10:21");
        assert_eq!(
            rooms[2].people[0].name, "Ann",
            "who is in voice shows without joining"
        );
        assert!(rooms[2].people[0].muted && rooms[3].people.is_empty());
        assert!(rooms.iter().all(|room| room.category.is_empty()));
        assert!(!rooms[0].voice && !rooms[0].encrypted && !rooms[0].read_only);
        assert!(rooms[1].read_only && !rooms[1].locked);
        assert_eq!(rooms[2].id, "discord:10:11");
        assert!(rooms[2].voice && !rooms[2].locked);
        assert!(rooms[3].locked);
        assert_eq!(rooms.len(), 4, "hidden channels stay hidden");
        assert_eq!(servers[0].id, "discord:10");
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
