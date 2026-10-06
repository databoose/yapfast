//! Discord servers and voice channels in Talk's lists. They appear as servers
//! and voice channels with ids that can't collide with Matrix ones, so the
//! rail, channel list, voice dock and the one-call rule cover them unchanged.
use talk_discord::{ready::Access, service::Snapshot};

use crate::chat::{ChatRoom, ChatServer};

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

/// The Discord server and channel of a voice channel's room id.
pub fn parse_room(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(PREFIX)?.split_once(':')
}

/// The servers and voice channels to list. They stay listed while Discord
/// reconnects; joining waits for the connection. Channels this account can see
/// but not join are listed locked, as in Discord; channels hidden from it are
/// listed locked only with `show_hidden`.
pub fn index(snapshot: &Snapshot, show_hidden: bool) -> (Vec<ChatRoom>, Vec<ChatServer>) {
    let mut rooms = Vec::new();
    let mut servers = Vec::new();
    for server in &snapshot.servers {
        let mut channels = Vec::new();
        for channel in &server.voice_channels {
            if channel.access == Access::Hidden && !show_hidden {
                continue;
            }
            let id = room_id(&server.id, &channel.id);
            rooms.push(ChatRoom {
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
            });
            channels.push(id);
        }
        servers.push(ChatServer {
            id: server_id(&server.id),
            name: server.name.clone(),
            channels,
            discord: true,
        });
    }
    (rooms, servers)
}

#[cfg(test)]
mod tests {
    use talk_discord::ready::{Channel, Server};

    use super::*;

    #[test]
    fn lists_voice_channels_under_their_server() {
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
                voice_channels: vec![
                    channel("11", Access::Join),
                    channel("12", Access::Locked),
                    channel("13", Access::Hidden),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let (rooms, servers) = index(&snapshot, false);
        assert_eq!(rooms[0].id, "discord:10:11");
        assert!(rooms[0].voice && !rooms[0].locked);
        assert!(rooms[1].locked);
        assert_eq!(rooms.len(), 2, "hidden channels stay hidden");
        assert_eq!(servers[0].id, "discord:10");
        assert_eq!(servers[0].channels, ["discord:10:11", "discord:10:12"]);
        let (rooms, _) = index(&snapshot, true);
        assert!(rooms.len() == 3 && rooms[2].locked);
        assert_eq!(parse_room("discord:10:11"), Some(("10", "11")));
        assert_eq!(parse_room("!room:example.org"), None);
    }
}
