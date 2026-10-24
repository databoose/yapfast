//! Dragging a Discord server's channels and categories in its channel list.

use super::*;
use crate::chat::ChannelPlacement;
use yapfast_discord::ready::permission;

/// The channel list as it shows, by room id: #rules outside any category,
/// Talk with #general, #news and Lounge, Games with #chess and Arena, and
/// Empty, which locks @everyone out of sending.
const SHOWN: [&str; 9] = [
    "discord:10:5",
    "discord:10:4",
    "discord:10:11",
    "discord:10:12",
    "discord:10:14",
    "discord:10:6",
    "discord:10:21",
    "discord:10:22",
    "discord:10:7",
];

/// The server with `me` in it, holding `roles`; user 1 owns it.
fn server(me: &str, roles: &[&str]) -> Application {
    use yapfast_discord::service::{Snapshot, Status};
    let bits = |bits: u64| bits.to_string();
    let mut server = yapfast_discord::ready::server(&serde_json::json!({
        "id": "10", "owner_id": "1",
        "properties": {"name": "Test"},
        "roles": [
            {"id": "10", "name": "@everyone", "position": 0,
             "permissions": bits(permission::VIEW_CHANNEL | permission::SEND_MESSAGES)},
            {"id": "20", "name": "Games mods", "position": 1, "permissions": "0"},
        ],
        "channels": [
            {"id": "5", "type": 0, "name": "rules", "position": 0},
            {"id": "4", "type": 4, "name": "Talk", "position": 0},
            {"id": "11", "type": 0, "name": "general", "position": 1, "parent_id": "4"},
            {"id": "12", "type": 0, "name": "news", "position": 2, "parent_id": "4"},
            {"id": "14", "type": 2, "name": "Lounge", "position": 0, "parent_id": "4"},
            {"id": "6", "type": 4, "name": "Games", "position": 1,
             "permission_overwrites": [{"id": "20", "type": 0,
                                        "allow": bits(permission::MANAGE_CHANNELS), "deny": "0"}]},
            {"id": "21", "type": 0, "name": "chess", "position": 3, "parent_id": "6",
             "permission_overwrites": [{"id": "20", "type": 0,
                                        "allow": bits(permission::MANAGE_CHANNELS), "deny": "0"}]},
            {"id": "22", "type": 2, "name": "Arena", "position": 1, "parent_id": "6",
             "permission_overwrites": [{"id": "20", "type": 0,
                                        "allow": bits(permission::MANAGE_CHANNELS), "deny": "0"}]},
            {"id": "7", "type": 4, "name": "Empty", "position": 2,
             "permission_overwrites": [{"id": "10", "type": 0, "allow": "0",
                                        "deny": bits(permission::SEND_MESSAGES)}]},
        ],
    }))
    .unwrap();
    server.my_roles = roles.iter().map(|role| (*role).to_owned()).collect();
    let mut app = Application::default();
    app.apply(discord_event(Snapshot {
        status: Status::Online,
        user_id: Some(me.into()),
        servers: vec![Arc::new(server)],
        ..Default::default()
    }));
    app
}

fn shown() -> Vec<String> {
    SHOWN.iter().map(|room| (*room).to_owned()).collect()
}

/// Where the row `dragged` lands dropped on the row `onto`.
fn landing(app: &Application, dragged: usize, onto: usize) -> Option<Landing> {
    app.landing(&shown(), dragged, onto)
}

fn room(id: &str) -> String {
    format!("discord:10:{id}")
}

/// What dropping the row `dragged` on the row `onto` asks Discord for,
/// keeping a channel's permissions if asked: each channel's or category's
/// id, position, and new category's id.
fn moved(
    app: &mut Application,
    dragged: usize,
    onto: usize,
) -> Vec<(String, i64, Option<Option<String>>)> {
    let landing = landing(app, dragged, onto).expect("it lands");
    let mut effects = app.dispatch(Action::MoveChannel {
        room: SHOWN[dragged].into(),
        landing,
    });
    if app.state.move_sync.is_some() {
        effects = app.dispatch(Action::AnswerMoveSync(Some(false)));
    }
    let [
        Effect::Backend(Command::Discord(DiscordCommand::MoveChannels {
            server,
            placements,
            lock: None,
        })),
    ] = &effects[..]
    else {
        panic!("not a move: {} effects", effects.len());
    };
    assert_eq!(server, "discord:10");
    placements
        .iter()
        .map(
            |ChannelPlacement {
                 room,
                 position,
                 parent,
             }| {
                let id = |room: &str| room.rsplit(':').next().unwrap().to_owned();
                (
                    id(room),
                    *position,
                    parent.as_ref().map(|parent| parent.as_deref().map(id)),
                )
            },
        )
        .collect()
}

fn at(id: &str, position: i64) -> (String, i64, Option<Option<String>>) {
    (id.into(), position, None)
}

fn into(id: &str, position: i64, category: Option<&str>) -> (String, i64, Option<Option<String>>) {
    (id.into(), position, Some(category.map(str::to_owned)))
}

#[test]
fn a_channel_dragged_onto_another_of_its_kind_takes_its_place() {
    let mut app = server("1", &[]);
    assert_eq!(landing(&app, 3, 2), Some(Landing::Before(room("11"))));
    assert_eq!(moved(&mut app, 3, 2), [at("12", 1), at("11", 2)]);
    // From another category, it joins that one.
    assert_eq!(landing(&app, 6, 3), Some(Landing::Before(room("12"))));
    assert_eq!(
        moved(&mut app, 6, 3),
        [into("21", 2, Some("4")), at("12", 3)]
    );
    // Dragged down, it goes below.
    assert_eq!(landing(&app, 2, 3), Some(Landing::After(room("12"))));
    assert_eq!(moved(&mut app, 2, 3), [at("12", 1), at("11", 2)]);
    assert_eq!(landing(&app, 2, 2), None, "onto itself");
}

#[test]
fn a_channel_dragged_onto_a_category_goes_first_in_it_or_last_above_it() {
    let mut app = server("1", &[]);
    // Down onto Games: first in it.
    assert_eq!(landing(&app, 2, 5), Some(Landing::First(Some(room("6")))));
    assert_eq!(
        moved(&mut app, 2, 5),
        [at("12", 1), into("11", 2, Some("6"))]
    );
    // Up onto Talk, which only #rules is above: last outside any category.
    assert_eq!(landing(&app, 4, 1), Some(Landing::Last(None)));
    assert_eq!(moved(&mut app, 4, 1), [into("14", 0, None)]);
    // Up onto the first row: first of all.
    assert_eq!(landing(&app, 2, 0), Some(Landing::Before(room("5"))));
    // Down onto the empty category.
    assert_eq!(landing(&app, 0, 8), Some(Landing::First(Some(room("7")))));
    // A text channel doesn't go up onto Games, which Lounge, a voice
    // channel, is above: it would land among the text channels, not there.
    assert_eq!(landing(&app, 6, 5), None);
}

#[test]
fn text_and_voice_channels_keep_to_their_own_lists() {
    let mut app = server("1", &[]);
    assert_eq!(landing(&app, 3, 4), None, "text down onto voice");
    assert_eq!(landing(&app, 4, 3), None, "voice up onto text");
    // A voice channel dragged down onto a category's last text channel goes
    // first of its voice channels.
    assert_eq!(landing(&app, 4, 6), Some(Landing::First(Some(room("6")))));
    assert_eq!(moved(&mut app, 4, 6), [into("14", 0, Some("6"))]);
}

#[test]
fn a_category_goes_above_another_or_below_its_last_row() {
    let mut app = server("1", &[]);
    assert_eq!(landing(&app, 5, 1), Some(Landing::Before(room("4"))));
    // The channels follow their categories, numbered again across the
    // server.
    assert_eq!(
        moved(&mut app, 5, 1),
        [
            at("6", 0),
            at("4", 1),
            at("21", 1),
            at("11", 2),
            at("12", 3),
            at("22", 0),
            at("14", 1)
        ]
    );
    assert_eq!(landing(&app, 1, 7), Some(Landing::After(room("6"))));
    assert_eq!(landing(&app, 1, 6), None, "not Games's last row");
    assert_eq!(landing(&app, 1, 4), None, "its own last row");
    assert_eq!(landing(&app, 5, 0), None, "among the channels outside any");
    assert_eq!(landing(&app, 1, 8), Some(Landing::After(room("7"))));
}

#[test]
fn moving_a_channel_into_a_category_asks_to_take_its_permissions() {
    let mut app = server("1", &[]);
    // #rules had no category, and Empty's permissions differ.
    let landing = landing(&app, 0, 8).unwrap();
    assert!(
        app.dispatch(Action::MoveChannel {
            room: SHOWN[0].into(),
            landing: landing.clone(),
        })
        .is_empty()
    );
    let asked = app.state.move_sync.as_ref().unwrap();
    assert_eq!(
        (asked.channel.as_str(), asked.category.as_str()),
        ("rules", "Empty")
    );
    let effects = app.dispatch(Action::AnswerMoveSync(Some(true)));
    let [
        Effect::Backend(Command::Discord(DiscordCommand::MoveChannels {
            lock, placements, ..
        })),
    ] = &effects[..]
    else {
        panic!("not a move: {} effects", effects.len());
    };
    assert_eq!(lock.as_deref(), Some(SHOWN[0]));
    assert_eq!(placements.last().unwrap().parent, Some(Some(room("7"))));
    assert!(app.state.move_sync.is_none());

    // Closed, it stays.
    app.dispatch(Action::MoveChannel {
        room: SHOWN[0].into(),
        landing,
    });
    assert!(app.dispatch(Action::AnswerMoveSync(None)).is_empty());
    assert!(app.state.move_sync.is_none());
}

#[test]
fn dragging_takes_manage_channels_where_the_channel_is_and_where_it_goes() {
    let app = server("2", &[]);
    assert!(SHOWN.iter().all(|room| !app.draggable(room)));
    assert!(
        app.droppable(&shown(), 2)
            .iter()
            .all(|droppable| !droppable)
    );

    // Games's moderators manage its channels, and nothing else.
    let mut app = server("2", &["20"]);
    let draggable: Vec<_> = SHOWN.iter().map(|room| app.draggable(room)).collect();
    assert_eq!(
        draggable,
        [false, false, false, false, false, false, true, true, false]
    );
    assert_eq!(
        app.droppable(&shown(), 6),
        [
            false, false, false, false, false, false, false, false, false
        ]
    );
    assert_eq!(
        app.droppable(&shown(), 7),
        [
            false, false, false, false, false, false, false, false, false
        ],
        "voice has nowhere else in Games"
    );
    assert!(app.landing(&shown(), 6, 3).is_none(), "Talk isn't theirs");
    assert!(
        app.dispatch(Action::MoveChannel {
            room: SHOWN[6].into(),
            landing: Landing::Before(room("12")),
        })
        .is_empty()
    );
}
