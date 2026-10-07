//! The preview, which shows Talk with sample rooms and no account.

use super::*;

impl Application {
    pub(super) fn start_preview(&mut self) -> Vec<Effect> {
        if self.state.busy || self.state.account.is_some() || self.state.preview {
            return vec![];
        }
        self.state.preview = true;
        self.state.servers = vec![
            ChatServer {
                id: "friends".into(),
                name: "Friends".into(),
                channels: vec![
                    "general".into(),
                    "games".into(),
                    "weekend".into(),
                    "lounge".into(),
                ],
                discord: false,
                muted: false,
                unavailable: false,
                icon: None,
            },
            ChatServer {
                id: "coop".into(),
                name: "Co-op crew".into(),
                channels: vec![
                    "announcements".into(),
                    "looking-for-group".into(),
                    "party-voice".into(),
                ],
                discord: false,
                muted: false,
                unavailable: false,
                icon: None,
            },
        ];
        self.state.navigation = Navigation::Server("friends".into());
        self.state.rooms = vec![
            preview_room("general", "general", false, 4),
            preview_room("games", "games", false, 4),
            preview_room("weekend", "weekend plans", false, 4),
            preview_room("announcements", "announcements", false, 3),
            preview_room("looking-for-group", "looking-for-group", false, 3),
            ChatRoom {
                voice: true,
                ..preview_room("lounge", "Lounge", false, 4)
            },
            ChatRoom {
                voice: true,
                ..preview_room("party-voice", "Party", false, 3)
            },
            preview_room("sam", "Sam", true, 2),
            preview_room("riley", "Riley", true, 2),
            preview_room("party", "Friday crew", true, 3),
        ];
        for (room, lines) in [
            (
                "announcements",
                vec![(
                    "Riley",
                    "Welcome to Co-op crew. This server has its own channels.",
                )],
            ),
            (
                "looking-for-group",
                vec![("Sam", "Looking for one more for co-op tonight.")],
            ),
            (
                "general",
                vec![
                    ("Sam", "Anyone around tonight?"),
                    (
                        "Sam",
                        "Thinking **co-op** after nine, if people are up for it.",
                    ),
                    ("Riley", "Yep. I should be free after dinner."),
                    ("You", "Nice, let's get a game going."),
                    (
                        "Sam",
                        "This is the local preview. Pick a conversation on the left, or send a message below to try it out.",
                    ),
                ],
            ),
            (
                "games",
                vec![
                    ("Riley", "What are we playing next?"),
                    ("Sam", "Something co-op. I'm open to suggestions."),
                ],
            ),
            (
                "sam",
                vec![(
                    "Sam",
                    "Hey! This is a sample DM. Your preview messages stay on this computer.",
                )],
            ),
            (
                "party",
                vec![
                    ("Riley", "Who's joining on Friday?"),
                    ("Sam", "Count me in."),
                ],
            ),
        ] {
            self.samples.insert(
                room.into(),
                lines
                    .into_iter()
                    .enumerate()
                    .map(|(i, (author, body))| ChatMessage {
                        id: format!("{room}-{i}"),
                        item: format!("{room}-{i}"),
                        author: author.into(),
                        sender: author.into(),
                        formatted: has_markdown(body),
                        body: body.into(),
                        time: format!("19:{:02}", 20 + i),
                        own: author == "You",
                        can_edit: author == "You",
                        can_delete: author == "You",
                        ..sample_actions()
                    })
                    .collect(),
            );
        }
        self.select("general".into())
    }

    /// The preview's devices as listed for it; nothing else about voice is real.
    pub fn preview_devices(
        &mut self,
        devices: Result<(Vec<voice::Device>, Vec<voice::Device>), String>,
    ) {
        if !self.state.preview {
            return;
        }
        match devices {
            Ok((microphones, speakers)) => {
                self.state.voice.microphones = microphones;
                self.state.voice.speakers = speakers;
            }
            Err(error) => self.state.voice.error = Some(error),
        }
    }

    /// A preview call's starting state, keeping the devices listed and chosen so far.
    pub(super) fn preview_voice(&self) -> voice::State {
        let voice = &self.state.voice;
        voice::State {
            microphones: voice.microphones.clone(),
            speakers: voice.speakers.clone(),
            microphone: voice.microphone.clone(),
            speaker: voice.speaker.clone(),
            ..Default::default()
        }
    }
}

pub(super) fn preview_room(id: &str, name: &str, direct: bool, members: u64) -> ChatRoom {
    ChatRoom {
        id: id.into(),
        name: name.into(),
        direct,
        voice: false,
        encrypted: true,
        members,
        unread: 0,
        mentions: 0,
        locked: false,
        read_only: false,
        category: String::new(),
        request: false,
        person: (direct && members <= 2).then(|| format!("@{id}:preview")),
        people: vec![],
        ringing: vec![],
        incoming: false,
        avatar: None,
        upload_limit: 50 * 1024 * 1024,
        presence: None,
        activity: None,
    }
}

/// Sample members whose names match the preview's messages.
pub(super) fn preview_members(room: &str) -> Vec<ChatMember> {
    use Role::{Admin, Member, Moderator};
    let people: &[(&str, Role)] = match room {
        "announcements" | "looking-for-group" => {
            &[("Riley", Admin), ("Sam", Member), ("You", Member)]
        }
        "party" => &[("Riley", Member), ("Sam", Member), ("You", Member)],
        "sam" => &[("Sam", Member), ("You", Member)],
        "riley" => &[("Riley", Member), ("You", Member)],
        _ => &[
            ("You", Admin),
            ("Sam", Moderator),
            ("Riley", Member),
            ("Morgan", Member),
        ],
    };
    let mut members: Vec<_> = people
        .iter()
        .map(|&(name, role)| ChatMember {
            id: format!("@{}:preview.local", name.to_lowercase()),
            name: name.into(),
            role,
            avatar: None,
            group: None,
            presence: None,
            color: None,
            activity: None,
        })
        .collect();
    chat::sort_members(&mut members);
    members
}

/// Preview messages support every local action; sent ones are also editable.
pub(super) fn sample_actions() -> ChatMessage {
    ChatMessage {
        can_reply: true,
        can_edit: true,
        can_delete: true,
        can_react: true,
        can_forward: true,
        ..Default::default()
    }
}
