//! A Discord server to check a member's menu in, for this crate's tests and
//! other crates' through the `test-fixtures` feature.

use std::sync::RwLock;

use serde_json::json;
use talk_discord::{
    message::Names,
    ready::{self, permission},
    service::{SharedNames, Snapshot, Status},
};

use super::*;

/// Alice, "1", is Staff in a server Owen, "99", owns. Her roles let her
/// kick, ban, time out, rename and manage roles below Staff. Cara, a Lead,
/// is above her; Dave, her friend, has no role; Gus holds Wardens, a low
/// role with Administrator; Hal is timed out. Everyone may change their own
/// nickname, and Alice has #general open, "discord:10:11", in the server
/// "discord:10". `alice` names her roles: "30" for Staff.
pub fn member_server(alice: &[&str]) -> Application {
    let bits = |bits: u64| bits.to_string();
    let staff = permission::KICK_MEMBERS
        | permission::BAN_MEMBERS
        | permission::MODERATE_MEMBERS
        | permission::MANAGE_ROLES
        | permission::MANAGE_NICKNAMES;
    let mut server = ready::server(&json!({
        "id": "10", "owner_id": "99",
        "properties": {"name": "Test"},
        "roles": [
            {"id": "10", "name": "@everyone", "position": 0, "permissions": bits(
                permission::VIEW_CHANNEL | permission::SEND_MESSAGES | permission::CHANGE_NICKNAME)},
            {"id": "60", "name": "Wardens", "position": 1, "permissions": bits(permission::ADMINISTRATOR)},
            {"id": "25", "name": "Bot", "position": 2, "permissions": "0", "managed": true},
            {"id": "20", "name": "Crew", "position": 3, "permissions": "0", "color": 0x3498db},
            {"id": "30", "name": "Staff", "position": 4, "permissions": bits(staff)},
            {"id": "40", "name": "Leads", "position": 5, "permissions": "0"},
        ],
        "channels": [{"id": "11", "type": 0, "name": "general", "position": 0}],
    }))
    .unwrap();
    server.my_roles = alice.iter().map(|role| (*role).to_owned()).collect();
    let mut names = Names::default();
    let soon = chrono::Utc::now() + chrono::Duration::hours(1);
    for (user, name, roles, nick, until) in [
        ("1", "Alice", alice.to_vec(), Some("Al"), None),
        ("3", "Cara", vec!["40"], None, None),
        ("4", "Dave", vec!["20"], None, None),
        ("6", "Gus", vec!["60"], None, None),
        ("7", "Hal", vec![], None, Some(soon.to_rfc3339())),
        ("99", "Owen", vec![], None, None),
    ] {
        names.learn_member(
            "10",
            &json!({
                "user": {"id": user, "username": name.to_lowercase(), "global_name": name},
                "roles": roles, "nick": nick, "communication_disabled_until": until,
            }),
        );
    }
    let mut app = Application::default();
    app.apply(Event::Discord(Arc::new(Snapshot {
        status: Status::Online,
        user_id: Some("1".into()),
        servers: vec![Arc::new(server)],
        friends: vec!["4".into()],
        friend_list: vec![talk_discord::service::Friend {
            user_id: "4".into(),
            name: "Dave".into(),
            avatar: String::new(),
            nickname: None,
            presence: None,
            activity: None,
        }],
        names: SharedNames::new(Arc::new(RwLock::new(names))),
        ..Default::default()
    })));
    app.dispatch(Action::Navigate(Navigation::Server("discord:10".into())));
    app.dispatch(Action::SelectRoom("discord:10:11".into()));
    let listed = |id: &str, name: &str| ChatMember {
        id: id.into(),
        name: name.into(),
        username: None,
        role: Role::Member,
        avatar: None,
        group: None,
        presence: None,
        color: None,
        activity: None,
        roles: Default::default(),
        timed_out_until: None,
    };
    app.apply(Event::Members {
        room: "discord:10:11".into(),
        members: Ok(["Al:1", "Cara:3", "Dave:4", "Gus:6", "Hal:7", "Owen:99"]
            .iter()
            .map(|entry| {
                let (name, id) = entry.split_once(':').unwrap();
                listed(id, name)
            })
            .collect()),
    });
    app
}
