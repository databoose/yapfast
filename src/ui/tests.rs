use super::*;

fn message_row_plain(message: &ChatMessage) -> MessageRow {
    message_row(message, &HashSet::new())
}

#[test]
fn equal_lists_stay_so_rows_count_as_unchanged() {
    let shown = ModelRc::new(VecModel::from(vec![1, 2]));
    let kept = same_list(&shown, ModelRc::new(VecModel::from(vec![1, 2])));
    assert!(kept == shown, "the list on screen stays");
    let changed = same_list(&shown, ModelRc::new(VecModel::from(vec![1, 3])));
    assert!(changed != shown);
    assert_eq!(changed.row_data(1), Some(3));

    // A profile built again from the same view isn't set again.
    let view = talk_core::application::ProfileView {
        profile: talk_core::chat::Profile {
            id: "@sam:test".into(),
            name: "Sam".into(),
            servers: vec![talk_core::chat::ProfileServer {
                id: "s".into(),
                name: "Server".into(),
                icon: None,
                banner: None,
                members: Some(4),
                channels: 2,
                nick: None,
            }],
            roles: vec![ProfileRole {
                name: "Mod".into(),
                color: Some(0xff0000),
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    let state = talk_core::application::AppState::default();
    let mut pictures = HashMap::new();
    let shown = profile_data(&mut pictures, &state, &view);
    let again = profile_data(&mut pictures, &state, &view);
    assert!(changed_profile(&shown, again).is_none());
    let mut renamed = view.clone();
    renamed.profile.name = "Samuel".into();
    let changed = changed_profile(&shown, profile_data(&mut pictures, &state, &renamed))
        .expect("a new name shows");
    assert!(changed.servers == shown.servers, "its lists stay");
}

#[test]
fn blank_lines_between_lines_show() {
    assert_eq!(keep_blank_lines("a\n\nb"), "a\n\u{a0}\nb");
    assert_eq!(keep_blank_lines("a\n\n\nb"), "a\n\u{a0}\n\u{a0}\nb");
    assert_eq!(keep_blank_lines("a\nb"), "a\nb");
    assert_eq!(
        keep_blank_lines("\n\na\n\n"),
        "\n\na\n\n",
        "only between lines"
    );
    assert_eq!(
        keep_blank_lines("a\n\n```\nx\n\ny\n```\n\nb"),
        "a\n\u{a0}\n```\nx\n\ny\n```\n\u{a0}\nb",
        "code keeps its lines"
    );
    // A blank line shows as a line of its own, unlike a line break.
    assert_ne!(markdown("one\n\ntwo"), markdown("one\ntwo"));
}

#[test]
fn the_composer_sends_on_enter_and_breaks_lines_on_shift_enter() {
    use slint::platform::{Key, WindowEvent};
    let window = one_message("Hi");
    let sent = Rc::new(Cell::new(0));
    let count = sent.clone();
    window.on_send(move || count.set(count.get() + 1));
    window.invoke_focus_composer();
    let press = |text: SharedString| {
        window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    };
    press("a".into());
    window.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    press(Key::Return.into());
    window.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    press("b".into());
    assert_eq!(window.get_draft(), "a\nb");
    assert_eq!(sent.get(), 0);
    press(Key::Return.into());
    assert_eq!(sent.get(), 1);
    assert_eq!(
        window.get_draft(),
        "a\nb",
        "sending is the application's to clear"
    );
}

#[test]
fn the_keyboard_steps_through_messages_and_acts_on_them() {
    use slint::platform::{Key, WindowEvent};
    let window = showing(
        ["One", "Two", "Three"]
            .into_iter()
            .map(|body| ChatMessage {
                author: "Sam".into(),
                body: body.into(),
                can_reply: true,
                ..Default::default()
            })
            .collect(),
    );
    let replied = Rc::new(RefCell::new(String::new()));
    let reply = replied.clone();
    window.on_reply(move |item| *reply.borrow_mut() = item.into());
    let edits = Rc::new(Cell::new(0));
    let count = edits.clone();
    window.on_edit_last(move || count.set(count.get() + 1));
    let key = |text: SharedString, pressed: bool| {
        window.window().dispatch_event(if pressed {
            WindowEvent::KeyPressed { text }
        } else {
            WindowEvent::KeyReleased { text }
        });
    };
    let press = |text: SharedString| {
        key(text.clone(), true);
        key(text, false);
    };
    window.invoke_focus_composer();
    // Up in the empty composer edits; Alt+Up goes to the newest message.
    press(Key::UpArrow.into());
    assert_eq!(edits.get(), 1);
    key(Key::Alt.into(), true);
    press(Key::UpArrow.into());
    key(Key::Alt.into(), false);
    assert_eq!(window.get_active_item(), "i2");
    press(Key::UpArrow.into());
    press(Key::UpArrow.into());
    press(Key::UpArrow.into());
    assert_eq!(window.get_active_item(), "i0", "the first stays");
    press(Key::DownArrow.into());
    assert_eq!(window.get_active_item(), "i1");
    press("r".into());
    assert_eq!(*replied.borrow(), "i1");
    // The reply goes back to the composer, so letters type again.
    press("x".into());
    assert_eq!(window.get_draft(), "x");
}

#[test]
fn preferences_set_the_clock_text_size_previews_and_unmuting() {
    i_slint_backend_testing::init_no_event_loop();
    let message = ChatMessage {
        timestamp: 1_791_329_400,
        time: "23:30".into(),
        embeds: vec![talk_core::chat::Embed::default()],
        ..Default::default()
    };
    // Missing preferences take their defaults: 24 hours, default text, previews.
    let defaults: Layout = serde_json::from_str("{}").unwrap();
    assert_eq!(
        defaults.noise_suppression,
        talk_core::noise::Suppressor::DeepFilterNet
    );
    let row = defaults.restyle(&message, message_row(&message, &HashSet::new()));
    assert_eq!(
        row.time,
        talk_core::clock::time_on(message.timestamp, false)
    );
    assert_eq!(row.embeds.row_count(), 1);
    let layout = Layout {
        twelve_hour_clock: true,
        text_size: Some(2),
        link_previews: Some(false),
        unmute_on_join: true,
        noise_suppression: talk_core::noise::Suppressor::RNNoise,
        noise_strength: talk_core::noise::Strength::Light,
        input_sensitivity: Some(-45),
        reduce_room_echo: true,
        ..Default::default()
    };
    let row = layout.restyle(&message, message_row(&message, &HashSet::new()));
    let time = talk_core::clock::time_on(message.timestamp, true);
    assert!(time.ends_with(" AM") || time.ends_with(" PM"));
    assert_eq!(row.time, time);
    assert_eq!(row.time_styled, slint::StyledText::from_plain_text(&time));
    assert_eq!(row.short_time, time.split(' ').next().unwrap());
    assert_eq!(row.embeds.row_count(), 0);
    // A message that is only a card says it's hidden; one with text doesn't.
    assert_eq!(
        row.hidden_card,
        "A card, hidden while link previews are off."
    );
    let titled = ChatMessage {
        embeds: vec![talk_core::chat::Embed {
            title: "Server status".into(),
            ..Default::default()
        }],
        ..message.clone()
    };
    assert!(
        layout
            .restyle(&titled, message_row(&titled, &HashSet::new()))
            .hidden_card
            .contains("\"Server status\"")
    );
    let with_text = ChatMessage {
        body: "Look".into(),
        ..message.clone()
    };
    assert!(
        layout
            .restyle(&with_text, message_row(&with_text, &HashSet::new()))
            .hidden_card
            .is_empty()
    );

    let window = TalkWindow::new().unwrap();
    layout.apply(&window);
    assert!(window.get_twelve_hour_clock() && !window.get_link_previews());
    // Discord calls suppress noise with the chosen network, as far as chosen.
    assert_eq!(window.get_noise_suppression(), 1);
    assert_eq!(window.get_noise_strength(), 2);
    assert_eq!(
        talk_core::noise::chosen(),
        talk_core::noise::Suppressor::RNNoise
    );
    let saved = serde_json::to_value(&layout).unwrap();
    assert_eq!(saved["noise_suppression"], "rnnoise");
    assert_eq!(saved["noise_strength"], "light");
    assert_eq!(window.get_sensitivity(), -45);
    assert!(window.get_dereverb() && !defaults.reduce_room_echo);
    assert_eq!(defaults.sensitivity(), -60);
    let out_of_range = Layout {
        input_sensitivity: Some(12),
        ..Default::default()
    };
    assert_eq!(out_of_range.sensitivity(), 0);
    // The third choice is Off; an index out of range is the default.
    assert_eq!(chosen(&SUPPRESSORS, 2), talk_core::noise::Suppressor::Off);
    assert_eq!(chosen(&STRENGTHS, -1), talk_core::noise::Strength::Full);
    assert_eq!(window.get_message_font_size(), 18.);
    // Joining a call turns the microphone on once it connects.
    let unmuted = Rc::new(Cell::new(false));
    let record = unmuted.clone();
    window.on_join_unmuted(move || record.set(true));
    window.set_muted(true);
    window.set_voice_ready(true);
    slint::platform::update_timers_and_animations();
    assert!(unmuted.get());
}

#[test]
fn each_sounds_own_volume_counts_only_while_adjusting_each() {
    i_slint_backend_testing::init_no_event_loop();
    let defaults: Layout = serde_json::from_str("{}").unwrap();
    assert_eq!(defaults.sound_set, SoundSet::Felt);
    assert_eq!(defaults.sound_gain(Sound::Join), 1.0);
    let mut layout = Layout {
        sound_set: SoundSet::Rhodes,
        sound_volume: Some(50),
        each_sound: [(Sound::Mute, 50)].into(),
        ..Default::default()
    };
    assert!((layout.sound_gain(Sound::Mute) - 0.25).abs() < 1e-6);
    layout.adjust_each_sound = true;
    assert!((layout.sound_gain(Sound::Mute) - 0.0625).abs() < 1e-6);
    assert!((layout.sound_gain(Sound::Join) - 0.25).abs() < 1e-6);

    let window = TalkWindow::new().unwrap();
    layout.apply(&window);
    assert_eq!(window.get_sound_set(), 2);
    assert_eq!(window.get_sound_volume(), 50);
    assert!(window.get_adjust_each_sound());
    let each: Vec<i32> = window.get_each_sound().iter().collect();
    assert_eq!(each, [100, 100, 50, 100, 100, 100, 100, 100]);
    let saved = serde_json::to_value(&layout).unwrap();
    assert_eq!(saved["sound_set"], "rhodes");
    assert_eq!(saved["each_sound"]["mute"], 50);

    // Someone who turned off the desktop's sound for notifications keeps
    // messages quiet.
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join(LAYOUT),
        r#"{"notification_sound":false}"#,
    )
    .unwrap();
    let read = Layout::read(directory.path());
    assert_eq!(read.sound_gain(Sound::Message), 0.0);
    assert_eq!(read.sound_gain(Sound::Mention), 0.0);
    assert!(read.sound_gain(Sound::Join) > 0.0);
    let saved = serde_json::to_value(&read).unwrap();
    assert!(saved.get("notification_sound").is_none());
}

#[test]
fn sound_settings_are_remembered() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    bind_callbacks(&window, &adapter);
    window.set_sound_set(3);
    window.set_sound_volume(40);
    window.set_adjust_each_sound(true);
    window.invoke_preferences_changed();
    window.invoke_each_sound_changed(2, 30);
    window.invoke_each_sound_changed(6, 0);
    let read = Layout::read(directory.path());
    assert_eq!(read.sound_set, SoundSet::Glow);
    assert_eq!(read.sound_volume, Some(40));
    assert!(read.adjust_each_sound);
    assert_eq!(read.each_sound(Sound::Mute), 30);
    assert_eq!(read.sound_gain(Sound::Message), 0.0);
    let each: Vec<i32> = window.get_each_sound().iter().collect();
    assert_eq!(each[2], 30);
    // Back at 100%, a sound's own volume is forgotten.
    window.invoke_each_sound_changed(2, 100);
    assert!(
        !Layout::read(directory.path())
            .each_sound
            .contains_key(&Sound::Mute)
    );
}

#[test]
fn embed_fields_share_rows_three_inline_at_a_time() {
    let field = |name: &str, inline| EmbedField {
        name: name.into(),
        value: String::new(),
        inline,
    };
    let rows = field_rows(&[
        field("a", true),
        field("b", true),
        field("c", true),
        field("d", true),
        field("e", false),
        field("f", true),
    ]);
    let names: Vec<Vec<slint::StyledText>> = rows
        .iter()
        .map(|row| row.fields.iter().map(|field| field.name).collect())
        .collect();
    let expected: Vec<Vec<slint::StyledText>> = [&["a", "b", "c"][..], &["d"], &["e"], &["f"]]
        .iter()
        .map(|row| row.iter().map(|name| bold(name)).collect())
        .collect();
    assert_eq!(names, expected);
}

/// A window showing one styled message from Sam, with the composer.
fn one_message(body: &str) -> TalkWindow {
    messages(&[("Sam", body)])
}

/// A window showing styled messages, each starting a group, with the composer.
fn messages(lines: &[(&str, &str)]) -> TalkWindow {
    showing(
        lines
            .iter()
            .map(|(author, body)| ChatMessage {
                author: (*author).into(),
                body: (*body).into(),
                formatted: true,
                ..Default::default()
            })
            .collect(),
    )
}

/// A window showing these messages, each starting a group and sent at 19:2 and its
/// index, with the composer.
fn showing(chat: Vec<ChatMessage>) -> TalkWindow {
    i_slint_backend_testing::init_no_event_loop();
    let window = TalkWindow::new().unwrap();
    copy_whole_messages(&window);
    // Without the adapter's clipboard reading, Ctrl+V pastes text, as it
    // does when the clipboard holds no files or picture.
    let weak = window.as_weak();
    window.on_paste_into_composer(move || weak.unwrap().invoke_paste_composer_text());
    window.set_workspace(true);
    window.set_room_selected(true);
    window.set_room_ready(true);
    window.set_composer_enabled(true);
    // A device that may write, so the verification banner stays away.
    window.set_trusted(true);
    let rows: Vec<MessageRow> = chat
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            message_row_plain(&ChatMessage {
                id: format!("m{index}"),
                item: format!("i{index}"),
                time: format!("19:2{index}"),
                ..message
            })
        })
        .collect();
    window.set_messages(ModelRc::new(VecModel::from(rows)));
    window.show().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1120., 760.));
    settle(&window);
    window
}

fn settle(window: &TalkWindow) {
    for _ in 0..5 {
        slint::platform::update_timers_and_animations();
        let _ = window.invoke_timeline_extent();
    }
}

/// For each height down the window, presses there beside the text, drags up past the top
/// of the list, turning the wheel `wheel` times on the way, lets go and presses Ctrl+C;
/// then returns the clipboard. The last drag to start on a message starts at the end of
/// the lowest, so it selects up to the end of that message. The list returns to the newest
/// message before each.
fn copy_upwards(window: &TalkWindow, wheel: usize) -> Option<String> {
    use slint::platform::{Key, PointerEventButton, WindowEvent};
    let dispatch = |event| window.window().dispatch_event(event);
    let at = |x, y| slint::LogicalPosition::new(x, y);
    let button = PointerEventButton::Left;
    for y in (100..700).step_by(4) {
        window.invoke_scroll_to_latest();
        settle(window);
        let y = y as f32;
        dispatch(WindowEvent::PointerMoved {
            position: at(900., y),
        });
        dispatch(WindowEvent::PointerPressed {
            position: at(900., y),
            button,
        });
        dispatch(WindowEvent::PointerMoved {
            position: at(900., y - 8.),
        });
        dispatch(WindowEvent::PointerMoved {
            position: at(900., 60.),
        });
        for _ in 0..wheel {
            dispatch(WindowEvent::PointerScrolled {
                position: at(900., 60.),
                delta_x: 0.,
                delta_y: 400.,
            });
            settle(window);
        }
        dispatch(WindowEvent::PointerReleased {
            position: at(900., 60.),
            button,
        });
        dispatch(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        dispatch(WindowEvent::KeyPressed { text: "c".into() });
        dispatch(WindowEvent::KeyReleased { text: "c".into() });
        dispatch(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
    }
    // The composer's single line would turn line breaks into spaces, so
    // read the clipboard itself.
    i_slint_core::window::WindowInner::from_pub(window.window())
        .context()
        .platform()
        .clipboard_text(i_slint_core::platform::Clipboard::DefaultClipboard)
}

#[test]
fn a_selection_runs_across_messages_with_their_names_and_times() {
    let window = messages(&[
        ("Sam", "Thinking **co-op** after nine."),
        ("Riley", "Yep. I should be free after dinner."),
    ]);
    assert_eq!(
        copy_upwards(&window, 0).as_deref(),
        Some(
            "Sam 19:20\nThinking co-op after nine.\nRiley 19:21\nYep. I should be free after dinner."
        )
    );
}

#[test]
fn pictures_on_their_way_hold_their_place_as_skeletons() {
    let attachment = |name: &str, preview_failed| talk_core::chat::Attachment {
        name: name.into(),
        size: Some(1000),
        image: true,
        dimensions: Some((1600, 900)),
        preview: None,
        preview_failed,
        placement: Placement::Message,
        spoiler: false,
        video: false,
    };
    let row = message_row_plain(&ChatMessage {
        attachments: vec![attachment("wide.png", false)],
        ..Default::default()
    });
    let cell = row.images.row_data(0).unwrap();
    assert!(cell.loading);
    // As the 360 by 270 preview will be: 1600 by 900 scaled to fit.
    assert_eq!((row.gallery_width, row.gallery_height), (360, 203));
    assert_eq!(row.files.row_count(), 0, "not a file card meanwhile");
    // One whose preview failed is a file card instead.
    let row = message_row_plain(&ChatMessage {
        attachments: vec![attachment("broken.png", true)],
        ..Default::default()
    });
    assert_eq!(row.images.row_count(), 0);
    assert_eq!(row.files.row_count(), 1);
}

#[test]
fn a_selection_copies_the_pictures_it_runs_through() {
    let picture = Arc::new(Picture {
        width: 40,
        height: 30,
        rgba: vec![128; 40 * 30 * 4],
        ..Default::default()
    });
    let window = showing(vec![
        ChatMessage {
            author: "Sam".into(),
            body: "Look at this.".into(),
            attachments: vec![talk_core::chat::Attachment {
                name: "cat.png".into(),
                size: None,
                image: true,
                dimensions: Some((400, 300)),
                preview: Some(picture),
                preview_failed: false,
                placement: Placement::Message,
                spoiler: false,
                video: false,
            }],
            ..Default::default()
        },
        ChatMessage {
            author: "Riley".into(),
            body: "Nice.".into(),
            ..Default::default()
        },
    ]);
    assert_eq!(
        copy_upwards(&window, 0).as_deref(),
        Some("Sam 19:20\nLook at this.\n[Picture]\nRiley 19:21\nNice.")
    );
}

#[test]
fn a_selection_keeps_the_messages_it_scrolls_past() {
    let chat: Vec<ChatMessage> = (0..40)
        .map(|index| ChatMessage {
            author: "Sam".into(),
            body: format!("Message {index}."),
            ..Default::default()
        })
        .collect();
    let window = showing(chat);
    let expected: Vec<String> = (0..40)
        .map(|index| format!("Sam 19:2{index}\nMessage {index}."))
        .collect();
    assert_eq!(copy_upwards(&window, 40), Some(expected.join("\n")));
}

#[test]
fn a_click_on_a_link_in_selectable_text_opens_it() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let window = one_message("See https://example.com/talk");
    let opened = Rc::new(RefCell::new(Vec::<String>::new()));
    let record = opened.clone();
    window.on_open_link(move |link| record.borrow_mut().push(link.into()));
    let button = PointerEventButton::Left;
    // The link starts after "See ", a little way into the text column.
    for y in (100..700).step_by(4) {
        let position = slint::LogicalPosition::new(440., y as f32);
        window
            .window()
            .dispatch_event(WindowEvent::PointerPressed { position, button });
        window
            .window()
            .dispatch_event(WindowEvent::PointerReleased { position, button });
    }
    let opened = opened.borrow();
    assert!(!opened.is_empty(), "a click on the link opens it");
    assert!(opened.iter().all(|link| link == "https://example.com/talk"));
}

#[test]
fn styled_messages_select_by_dragging_and_copy_as_plain_text() {
    use slint::platform::{Key, PointerEventButton, WindowEvent};
    let window = one_message("Thinking **co-op** after nine, if people are up for it.");
    let dispatch = |event| window.window().dispatch_event(event);
    let button = PointerEventButton::Left;
    let at = |x, y| slint::LogicalPosition::new(x, y);
    let ctrl = |key: &str| {
        dispatch(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        dispatch(WindowEvent::KeyPressed { text: key.into() });
        dispatch(WindowEvent::KeyReleased { text: key.into() });
        dispatch(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
    };

    // Clicking the message without selecting leaves typing in the composer.
    let click = |x, y| {
        dispatch(WindowEvent::PointerPressed {
            position: at(x, y),
            button,
        });
        dispatch(WindowEvent::PointerReleased {
            position: at(x, y),
            button,
        });
    };
    let type_key = |key: &str| {
        dispatch(WindowEvent::KeyPressed { text: key.into() });
        dispatch(WindowEvent::KeyReleased { text: key.into() });
    };
    click(600., 712.);
    type_key("a");
    for y in (100..700).step_by(4) {
        click(500., y as f32);
    }
    type_key("b");
    assert_eq!(window.get_draft(), "ab");
    window.set_draft(SharedString::default());

    // Without element lookups in this build, drag down the timeline row by
    // row: from right of the text, past its end, to left of the timeline,
    // past its start. A drag that lands on the message selects all of it,
    // and copies it.
    for y in (100..700).step_by(4) {
        let y = y as f32;
        dispatch(WindowEvent::PointerMoved {
            position: at(900., y),
        });
        dispatch(WindowEvent::PointerPressed {
            position: at(900., y),
            button,
        });
        dispatch(WindowEvent::PointerMoved {
            position: at(600., y),
        });
        dispatch(WindowEvent::PointerMoved {
            position: at(200., y),
        });
        dispatch(WindowEvent::PointerReleased {
            position: at(200., y),
            button,
        });
        ctrl("c");
    }

    // Pasting shows what was copied: the text as read, without its Markdown.
    click(600., 712.);
    ctrl("v");
    assert_eq!(
        window.get_draft(),
        "Thinking co-op after nine, if people are up for it."
    );
}

#[test]
fn pointer_input_reaches_navigation_buttons() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    bind_callbacks(&window, &adapter);
    window.invoke_explore_preview();
    window.show().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1120., 760.));
    let click = |x, y| {
        let position = slint::LogicalPosition::new(x, y);
        let button = slint::platform::PointerEventButton::Left;
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
    };
    click(32., 36.);
    assert!(window.get_dms_selected());
    assert_eq!(window.get_sidebar_title(), "Direct messages");
    click(32., 101.);
    assert!(!window.get_dms_selected());
    assert_eq!(window.get_sidebar_title(), "Friends");
    window.hide().unwrap();
}

#[test]
fn menus_list_their_choices_and_carry_them_out() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    bind_callbacks(&window, &adapter);
    window.invoke_explore_preview();
    let texts = |window: &TalkWindow| -> Vec<String> {
        window
            .get_menu_rows()
            .iter()
            .map(|row| row.text.to_string())
            .collect()
    };
    window.invoke_prepare_menu("sam".into(), false);
    assert_eq!(texts(&window), ["Profile"]);
    window.invoke_prepare_menu("lounge".into(), false);
    assert_eq!(texts(&window), ["Join voice"]);
    window.invoke_prepare_menu("gone".into(), false);
    assert!(texts(&window).is_empty());

    window.invoke_choose_menu("sam".into(), false, "profile".into());
    assert!(adapter.borrow().app.state.profile.is_some());
    window.invoke_choose_menu("lounge".into(), false, "join-voice".into());
    assert_eq!(
        adapter.borrow().app.state.voice.room.as_deref(),
        Some("lounge")
    );

    let clipboard = || {
        i_slint_core::window::WindowInner::from_pub(window.window())
            .context()
            .platform()
            .clipboard_text(i_slint_core::platform::Clipboard::DefaultClipboard)
    };
    window.invoke_choose_menu("discord:10:21".into(), false, "copy-link".into());
    assert_eq!(
        clipboard().as_deref(),
        Some("https://discord.com/channels/10/21")
    );
    window.invoke_choose_menu("discord:10:21".into(), false, "copy-id".into());
    assert_eq!(clipboard().as_deref(), Some("21"));
    window.invoke_choose_menu("discord:10".into(), false, "copy-link".into());
    assert_eq!(
        clipboard().as_deref(),
        Some("https://discord.com/channels/10")
    );

    // Someone in the open conversation: their card, and the DM with them.
    window.invoke_navigate_server("friends".into());
    window.invoke_select_room("general".into());
    window.invoke_prepare_menu("@sam:preview.local".into(), true);
    assert_eq!(texts(&window), ["Profile", "Message"]);
    window.invoke_choose_menu("@sam:preview.local".into(), true, "message".into());
    assert_eq!(adapter.borrow().app.state.selected.as_deref(), Some("sam"));
}

#[test]
fn the_input_caret_opens_its_menu_and_measures_only_while_open() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    bind_callbacks(&window, &adapter);
    window.invoke_explore_preview();
    window.show().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1120., 760.));
    adapter
        .borrow_mut()
        .action(&window, Action::RequestJoinVoice("lounge".into()));
    let click = |x, y| {
        let position = slint::LogicalPosition::new(x, y);
        let button = slint::platform::PointerEventButton::Left;
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
    };
    // The caret sits right of the microphone toggle in the dock.
    click(206., 732.);
    assert!(adapter.borrow().meter.is_some());
    assert!(window.get_muted());
    click(800., 200.);
    assert!(adapter.borrow().meter.is_none());
    window.hide().unwrap();
}

#[test]
fn dragging_the_sidebar_edge_resizes_it_within_limits_and_remembers_it() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    bind_callbacks(&window, &adapter);
    window.invoke_explore_preview();
    window.show().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1120., 760.));
    let drag = |from: f32, to: f32| {
        let window = window.window();
        let button = slint::platform::PointerEventButton::Left;
        let at = |x| slint::LogicalPosition::new(x, 400.);
        window.dispatch_event(slint::platform::WindowEvent::PointerMoved { position: at(from) });
        window.dispatch_event(slint::platform::WindowEvent::PointerPressed {
            position: at(from),
            button,
        });
        window.dispatch_event(slint::platform::WindowEvent::PointerMoved { position: at(to) });
        window.dispatch_event(slint::platform::WindowEvent::PointerReleased {
            position: at(to),
            button,
        });
    };
    assert_eq!(window.get_sidebar_shown(), 240.);
    drag(312., 362.);
    assert_eq!(window.get_sidebar_shown(), 290.);
    assert_eq!(Layout::read(directory.path()).sidebar_width, Some(290.));
    // The sidebar stops at 380px, and at 180px.
    drag(362., 700.);
    assert_eq!(window.get_sidebar_shown(), 380.);
    drag(452., 0.);
    assert_eq!(window.get_sidebar_shown(), 180.);
    // In a narrow window, dragging keeps 420px for the conversation,
    // beside the member list when it shows.
    window
        .window()
        .set_size(slint::LogicalSize::new(800., 560.));
    drag(252., 700.);
    assert_eq!(window.get_sidebar_shown(), 180.);
    window.set_member_list(false);
    drag(252., 700.);
    assert_eq!(window.get_sidebar_shown(), 308.);
    drag(380., 252.);
    // A new window starts at the stored width.
    let next = TalkWindow::new().unwrap();
    adapter.borrow_mut().attach(&next);
    assert_eq!(next.get_sidebar_width(), 180.);
    window.hide().unwrap();
}

#[test]
fn the_member_list_shows_for_text_channels_and_remembers_being_closed() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    bind_callbacks(&window, &adapter);
    window.invoke_explore_preview();
    window.show().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1120., 760.));
    assert!(window.get_members_available() && window.get_member_list());
    let roster = window.get_roster();
    assert_eq!(roster.row_count(), 4);
    assert_eq!(roster.row_data(0).unwrap().heading, "ADMINS — 1");
    assert_eq!(roster.row_data(2).unwrap().heading, "MEMBERS — 2");
    assert!(roster.row_data(3).unwrap().heading.is_empty());
    let click = |x, y| {
        let position = slint::LogicalPosition::new(x, y);
        let button = slint::platform::PointerEventButton::Left;
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
    };
    // The toggle ends the conversation header.
    click(1080., 24.);
    assert!(!window.get_member_list());
    assert!(Layout::read(directory.path()).members_hidden);
    click(1080., 24.);
    assert!(window.get_member_list());
    // Voice channels and one-to-one DMs have no member list.
    window.invoke_select_room("lounge".into());
    assert!(!window.get_members_available());
    window.invoke_navigate_dms();
    assert!(!window.get_members_available());
    window.invoke_select_room("party".into());
    assert!(window.get_members_available());
    assert_eq!(window.get_roster().row_count(), 3);
    window.hide().unwrap();
}

#[test]
fn forwarding_and_frequent_reactions_reach_the_window() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    bind_callbacks(&window, &adapter);
    window.invoke_explore_preview();
    let glyphs =
        |model: ModelRc<EmojiCell>| model.iter().map(|cell| cell.glyph).collect::<Vec<_>>();
    assert_eq!(glyphs(window.get_bar_reactions()), ["👍", "❤️", "😂"]);
    let message = window.get_messages().row_data(0).unwrap();
    window.invoke_react(message.item.clone(), "🍕".into());
    assert_eq!(
        glyphs(window.get_menu_reactions()),
        ["🍕", "👍", "❤️", "😂"]
    );
    assert_eq!(Layout::read(directory.path()).reactions.get("🍕"), Some(&1));

    window.invoke_start_forward(message.item.clone());
    assert!(window.get_forward_open());
    assert_eq!(window.get_forward_preview(), "Anyone around tonight?");
    let targets = window.get_forward_targets();
    assert!(
        targets.iter().all(|target| target.id != "lounge"),
        "voice is no target"
    );
    window.set_forward_query("gam".into());
    window.invoke_forward_search();
    assert_eq!(window.get_forward_targets().row_count(), 1);
    window.invoke_forward_to("games".into());
    assert!(window.get_forward_targets().row_data(0).unwrap().sent);
    window.invoke_close_forward();
    assert!(!window.get_forward_open());

    window.invoke_emoji_search("pizza".into());
    assert_eq!(window.get_emoji_rows().row_count(), 1);
}

#[test]
fn scrolling_up_leaves_the_latest_message() {
    i_slint_backend_testing::init_no_event_loop();
    let window = TalkWindow::new().unwrap();
    window.set_workspace(true);
    window.set_room_selected(true);
    window.set_room_ready(true);
    let rows: Vec<MessageRow> = (0..200)
        .map(|index| MessageRow {
            id: format!("m{index}").into(),
            item: format!("i{index}").into(),
            author: "Bob".into(),
            body: if index % 2 == 0 {
                format!("Message {index}").into()
            } else {
                format!("Message {index}, a longer line that wraps across the timeline at ordinary window widths, and then some more words").into()
            },
            first: index % 5 == 0,
            ..Default::default()
        })
        .collect();
    window.set_messages(ModelRc::new(VecModel::from(rows)));
    window.show().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1120., 760.));
    let settle = || {
        for _ in 0..5 {
            slint::platform::update_timers_and_animations();
            let _ = window.invoke_timeline_extent();
        }
    };
    settle();
    window.invoke_scroll_to_latest();
    settle();
    assert!(
        window.invoke_timeline_at_end(),
        "starts at the latest message"
    );
    for _ in 0..8 {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(700., 380.),
                delta_x: 0.,
                delta_y: 50.,
            });
        settle();
    }
    assert!(
        !window.invoke_timeline_at_end(),
        "scrolling up leaves the latest message"
    );
    window.hide().unwrap();
}

#[test]
fn a_wheel_spun_fast_scrolls_farther_for_each_notch() {
    i_slint_backend_testing::init_no_event_loop();
    let window = TalkWindow::new().unwrap();
    window.set_workspace(true);
    window.set_room_selected(true);
    window.set_room_ready(true);
    window.set_history_start(true);
    let rows: Vec<MessageRow> = (0..300)
        .map(|index| {
            let mut row = message_row_plain(&ChatMessage {
                id: format!("m{index}"),
                item: format!("i{index}"),
                author: "Bob".into(),
                body: format!("Message {index}"),
                formatted: true,
                ..Default::default()
            });
            row.first = true;
            row
        })
        .collect();
    window.set_messages(ModelRc::new(VecModel::from(rows)));
    window.show().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1120., 760.));
    let wait = |time: Duration| {
        i_slint_backend_testing::mock_elapsed_time(time);
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(1100., 740.),
            });
    };
    // Scrolls up by ten notches, `apart` from each other, and tells how many
    // rows that went.
    let spin = |apart: Duration| {
        window.invoke_scroll_to_latest();
        for _ in 0..20 {
            wait(Duration::from_millis(50));
        }
        let start = window.get_top_index();
        for _ in 0..10 {
            // As the winit backend sends a notch: `PointerScrolled` would
            // jump without the wheel's animation.
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::internal(
                    i_slint_core::input::BackendMouseEvent::Wheel {
                        position: i_slint_core::lengths::LogicalPoint::new(700., 380.),
                        delta_x: 0.,
                        delta_y: 60.,
                        phase: i_slint_core::input::TouchPhase::Moved,
                    },
                ));
            wait(apart);
        }
        for _ in 0..20 {
            wait(Duration::from_millis(50));
        }
        start - window.get_top_index()
    };
    let slow = spin(Duration::from_millis(300));
    let fast = spin(Duration::from_millis(20));
    assert!(slow > 0);
    assert!(
        fast as f32 > slow as f32 * 1.5,
        "fast notches went {fast} rows, slow ones {slow}"
    );
    window.hide().unwrap();
}

#[test]
fn older_messages_go_in_above_without_moving_those_shown() {
    i_slint_backend_testing::init_no_event_loop();
    let window = TalkWindow::new().unwrap();
    window.set_workspace(true);
    window.set_room_selected(true);
    window.set_room_ready(true);
    window.set_loading_history(true);
    // The first row carries the loading notice and its day; once older
    // messages from that day are above it, it carries neither.
    let rows = |from: usize| -> Vec<MessageRow> {
        (from..200)
            .map(|index| {
                let body = if index % 3 == 0 {
                    format!("Message {index}, a longer line that wraps across the timeline at ordinary window widths, and then some more words")
                } else {
                    format!("Message {index}")
                };
                let mut row = message_row_plain(&ChatMessage {
                    id: format!("m{index}"),
                    item: format!("i{index}"),
                    author: "Bob".into(),
                    body,
                    formatted: true,
                    ..Default::default()
                });
                row.first = true;
                if index == from {
                    row.day = "1 October 2026".into();
                }
                row
            })
            .collect()
    };
    let model = Rc::new(VecModel::from(rows(150)));
    window.set_messages(ModelRc::from(model.clone()));
    window.show().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1120., 760.));
    // As drawing does, the pointer goes through the list, which lays it out.
    let settle = || {
        for _ in 0..5 {
            slint::platform::update_timers_and_animations();
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(1100., 740.),
                });
        }
    };
    settle();
    window.invoke_scroll_to_latest();
    settle();
    for _ in 0..40 {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(700., 380.),
                delta_x: 0.,
                delta_y: 400.,
            });
        settle();
    }
    // The first message, with the loading notice and its day above it, is
    // partly scrolled under the top.
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(700., 380.),
            delta_x: 0.,
            delta_y: -90.,
        });
    settle();
    assert_eq!(window.get_top_index(), 0);
    let edge = window.get_top_edge();
    show_rows(&model, rows(100));
    settle();
    assert_eq!(
        (window.get_top_index(), window.get_top_item().as_str()),
        (50, "i150"),
        "the same message is at the top"
    );
    assert!(
        (window.get_top_edge() - edge).abs() < 0.5,
        "its text stays where it was: {edge} then {}",
        window.get_top_edge()
    );
    assert!(!window.invoke_timeline_at_end());
    // A message deleted above leaves it where it is too.
    model.remove(10);
    settle();
    assert_eq!(
        (window.get_top_index(), window.get_top_item().as_str()),
        (49, "i150"),
        "the same message is at the top after a deletion above"
    );
    assert!(
        (window.get_top_edge() - edge).abs() < 0.5,
        "its text stays where it was after a deletion above: {edge} then {}",
        window.get_top_edge()
    );
    // Scrolling up shows those that went in.
    for _ in 0..2 {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(700., 380.),
                delta_x: 0.,
                delta_y: 200.,
            });
        settle();
    }
    assert!(window.get_top_index() < 50);
    window.hide().unwrap();
}

#[test]
fn scrolling_up_through_styled_messages_stays_up() {
    i_slint_backend_testing::init_no_event_loop();
    let window = TalkWindow::new().unwrap();
    window.set_workspace(true);
    window.set_room_selected(true);
    window.set_room_ready(true);
    let rows: Vec<MessageRow> = (0..200)
        .map(|index| {
            let body = match index % 4 {
                0 => format!("**Message** {index}"),
                1 => format!("Message {index} with https://example.com/{index} and words that wrap across the timeline at ordinary widths, and then *some* more"),
                2 => format!("line one {index}\nline two\nline three"),
                _ => format!("Message {index}"),
            };
            let mut row = message_row_plain(&ChatMessage {
                id: format!("m{index}"),
                item: format!("i{index}"),
                author: "Bob".into(),
                body,
                formatted: true,
                ..Default::default()
            });
            row.first = index % 5 == 0;
            row
        })
        .collect();
    window.set_messages(ModelRc::new(VecModel::from(rows)));
    window.show().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1120., 760.));
    let settle = || {
        for _ in 0..5 {
            slint::platform::update_timers_and_animations();
            let _ = window.invoke_timeline_extent();
        }
    };
    settle();
    window.invoke_scroll_to_latest();
    settle();
    assert!(
        window.invoke_timeline_at_end(),
        "starts at the latest message"
    );
    for step in 0..8 {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(700., 380.),
                delta_x: 0.,
                delta_y: 50.,
            });
        settle();
        assert!(
            !window.invoke_timeline_at_end() && window.invoke_timeline_below() > 0.,
            "step {step} stays scrolled up"
        );
    }
    // Scrolling back down resumes following, and later rows stay in view.
    for _ in 0..40 {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(700., 380.),
                delta_x: 0.,
                delta_y: -200.,
            });
        settle();
    }
    assert!(
        window.invoke_timeline_at_end(),
        "back at the latest message"
    );
    let messages = window.get_messages();
    let model = messages
        .as_any()
        .downcast_ref::<VecModel<MessageRow>>()
        .unwrap();
    model.push(message_row_plain(&ChatMessage {
        id: "new".into(),
        item: "new".into(),
        author: "Ann".into(),
        body: "A *new* message with https://example.com\nand a second line".into(),
        formatted: true,
        ..Default::default()
    }));
    settle();
    assert_eq!(window.invoke_timeline_below(), 0., "the new message shows");
    window.hide().unwrap();
}

#[test]
fn callbacks_preserve_drafts_scope_voice_and_clear_session_inputs() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    bind_callbacks(&window, &adapter);
    window.invoke_explore_preview();
    assert!(window.get_workspace() && window.get_preview());
    let server = adapter.borrow().app.state.servers[0].id.clone();
    window.invoke_navigate_server(server.clone().into());
    let rooms = adapter
        .borrow()
        .app
        .state
        .rooms
        .iter()
        .filter(|room| adapter.borrow().app.state.in_scope(room))
        .cloned()
        .collect::<Vec<_>>();
    let text_rooms = rooms.iter().filter(|room| !room.voice).collect::<Vec<_>>();
    window.invoke_select_room(text_rooms[0].id.clone().into());
    window.invoke_draft_edited("Draft survives navigation".into());
    window.invoke_select_room(text_rooms[1].id.clone().into());
    window.invoke_draft_edited("Different channel draft".into());
    window.invoke_select_room(text_rooms[0].id.clone().into());
    assert_eq!(window.get_draft(), "Draft survives navigation");
    assert!(window.get_can_send());
    let count = window.get_messages().row_count();
    window.invoke_send();
    assert_eq!(window.get_messages().row_count(), count + 1);
    assert!(window.get_draft().is_empty());
    let voice_room = rooms.iter().find(|room| room.voice).unwrap();
    // Clicking a voice channel joins it.
    window.invoke_select_room(voice_room.id.clone().into());
    assert!(window.get_voice_here() && window.get_voice_active() && window.get_muted());
    window.invoke_navigate_dms();
    assert!(window.get_voice_active() && window.get_voice_visible());
    assert!(!window.get_voice_here());
    assert!(window.get_rooms().iter().all(|row| {
        adapter
            .borrow()
            .app
            .state
            .rooms
            .iter()
            .any(|room| room.id.as_str() == row.id.as_str() && room.direct)
    }));
    window.invoke_toggle_mute();
    assert!(!window.get_muted());
    window.invoke_toggle_deafen();
    assert!(window.get_muted() && window.get_deafened());
    window.set_password("secret that must be cleared".into());
    window.set_recovery("recovery that must be cleared".into());
    window.set_settings_open(true);
    window.invoke_logout();
    assert!(!window.get_workspace() && !window.get_voice_active());
    assert!(window.get_password().is_empty() && window.get_recovery().is_empty());
    assert!(!window.get_settings_open());
}

#[test]
fn message_actions_reach_the_shared_application_in_the_preview() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    bind_callbacks(&window, &adapter);
    window.invoke_explore_preview();
    let row = |item: &SharedString| {
        window
            .get_messages()
            .iter()
            .find(|row| row.item == *item)
            .unwrap()
    };
    let own = window.get_messages().iter().find(|row| row.own).unwrap();
    let other = window.get_messages().iter().find(|row| !row.own).unwrap();
    assert!(own.can_edit && !other.can_edit && !window.get_can_attach());

    window.invoke_react(other.item.clone(), "👍".into());
    let reactions = row(&other.item).reactions;
    assert_eq!(reactions.row_count(), 1);
    assert!(reactions.row_data(0).unwrap().mine);
    window.invoke_react(other.item.clone(), "👍".into());
    assert_eq!(row(&other.item).reactions.row_count(), 0);

    window.invoke_reply(other.item.clone());
    assert!(window.get_compose_label().starts_with("Replying to Sam"));
    window.invoke_draft_edited("**Sure**".into());
    window.invoke_send();
    let reply = window.get_messages().iter().last().unwrap();
    assert!(reply.has_reply && reply.reply_author == "Sam");
    assert!(window.get_compose_label().is_empty() && window.get_draft().is_empty());

    window.invoke_draft_edited("Unsent draft".into());
    window.invoke_edit(own.item.clone());
    assert_eq!(window.get_draft(), own.body);
    assert_eq!(window.get_compose_label(), "Editing your message");
    window.invoke_draft_edited("Changed".into());
    window.invoke_send();
    let edited = row(&own.item);
    assert!(edited.edited && edited.body == "Changed");
    assert_eq!(window.get_draft(), "Unsent draft");

    window.invoke_request_delete(other.item.clone());
    assert!(
        window.get_confirm_delete().is_empty(),
        "only own messages can be deleted"
    );
    window.invoke_request_delete(own.item.clone());
    assert_eq!(window.get_confirm_delete(), own.item);
    window.invoke_confirm_delete_message();
    assert!(row(&own.item).deleted && window.get_confirm_delete().is_empty());
}

#[test]
fn signed_in_controls_keep_trust_and_plaintext_sending_gates() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    bind_callbacks(&window, &adapter);
    window.invoke_explore_preview();
    {
        let mut adapter = adapter.borrow_mut();
        adapter.app.state.preview = false;
        adapter.app.state.account = Some("@test:example.invalid".into());
        adapter.app.state.trusted = false;
        adapter.refresh(&window);
    }
    window.invoke_draft_edited("Keep encrypted".into());
    assert!(!window.get_can_send());
    window.invoke_send();
    assert!(!window.get_sending());
    {
        let mut adapter = adapter.borrow_mut();
        adapter.app.state.trusted = true;
        let id = adapter.app.state.selected.clone().unwrap();
        adapter
            .app
            .state
            .rooms
            .iter_mut()
            .find(|room| room.id == id)
            .unwrap()
            .encrypted = false;
        adapter.refresh(&window);
    }
    assert!(!window.get_composer_enabled() && !window.get_can_send());
    window.invoke_send();
    assert_eq!(window.get_draft(), "Keep encrypted");
    assert!(!window.get_sending());
}

/// What a list told the window about its rows, as `changed 3`, `added 0+2` and
/// `removed 1+1`.
#[derive(Default)]
struct Notices(RefCell<Vec<String>>);

impl i_slint_core::model::ModelChangeListener for Notices {
    fn row_changed(self: std::pin::Pin<&Self>, row: usize) {
        self.0.borrow_mut().push(format!("changed {row}"));
    }
    fn row_added(self: std::pin::Pin<&Self>, index: usize, count: usize) {
        self.0.borrow_mut().push(format!("added {index}+{count}"));
    }
    fn row_removed(self: std::pin::Pin<&Self>, index: usize, count: usize) {
        self.0.borrow_mut().push(format!("removed {index}+{count}"));
    }
    fn reset(self: std::pin::Pin<&Self>) {
        self.0.borrow_mut().push("reset".into());
    }
}

#[test]
fn rows_with_ids_change_only_where_they_changed() {
    use i_slint_core::model::ModelChangeListenerContainer;
    let model = VecModel::from(vec![("b", 1), ("c", 1), ("d", 1)]);
    let notices = Box::pin(ModelChangeListenerContainer::new(Notices::default()));
    model
        .model_tracker()
        .attach_peer(notices.as_ref().model_peer());
    let update = |rows: Vec<(&'static str, i32)>| {
        update_model_by_id(&model, rows.clone(), |row| row.0.into());
        assert_eq!(model.iter().collect::<Vec<_>>(), rows);
        std::mem::take(&mut *notices.0.borrow_mut())
    };
    // Older rows come in above, and those below stay as they were.
    assert_eq!(
        update(vec![("a", 1), ("b", 1), ("c", 1), ("d", 1)]),
        ["added 0+1"]
    );
    assert_eq!(
        update(vec![("a", 1), ("b", 1), ("c", 2), ("d", 1)]),
        ["changed 2"]
    );
    assert_eq!(
        update(vec![("a", 1), ("b", 1), ("c", 2), ("d", 1), ("e", 1)]),
        ["added 4+1"]
    );
    assert_eq!(
        update(vec![("a", 1), ("c", 2), ("d", 1), ("e", 1)]),
        ["removed 1+1"]
    );
    assert_eq!(
        update(vec![("a", 1), ("x", 1), ("y", 1), ("e", 2)]),
        [
            "removed 1+1",
            "removed 1+1",
            "added 1+1",
            "added 2+1",
            "changed 3"
        ]
    );
    assert_eq!(update(vec![]).len(), 4, "each row goes");
}

#[test]
fn typing_and_the_call_show_without_building_the_rest_again() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    bind_callbacks(&window, &adapter);
    window.invoke_explore_preview();
    let mut adapter = adapter.borrow_mut();
    let room = adapter.app.state.selected.clone().unwrap();
    let reach = adapter.apply(Event::Typing {
        room,
        names: vec!["Sam".into()],
    });
    assert!(
        !reach
            .and(adapter.apply(Event::Voice(Default::default())))
            .everything
    );
    adapter.show(&window, reach);
    assert_eq!(window.get_typing(), "Sam is typing...");
    // A refresh that changes nothing the sidebar shows doesn't build it again,
    // so a row marked here stays marked.
    let mut row = adapter.models.rooms.row_data(0).unwrap();
    row.name = "Marked".into();
    adapter.models.rooms.set_row_data(0, row);
    adapter.refresh(&window);
    assert_eq!(adapter.models.rooms.row_data(0).unwrap().name, "Marked");
    adapter.app.state.rooms[0].name = "Renamed".into();
    adapter.refresh(&window);
    assert_ne!(adapter.models.rooms.row_data(0).unwrap().name, "Marked");
}

#[test]
fn collapsed_categories_keep_the_open_unread_and_occupied_channels() {
    let row = |id: &str, heading: &str| RoomRow {
        id: id.into(),
        heading: heading.into(),
        ..Default::default()
    };
    let rows = vec![
        row("a", "TEXT"),
        RoomRow {
            selected: true,
            ..row("b", "")
        },
        RoomRow {
            unread: true,
            ..row("c", "")
        },
        row("d", ""),
        row("e", "VOICE"),
        row("f", ""),
        row("g", "OTHER"),
        row("h", ""),
    ];
    let collapsed = std::collections::BTreeSet::from(["TEXT".to_owned(), "VOICE".to_owned()]);
    let folded = refresh::fold_collapsed(rows, &collapsed, Some("f"));
    let shown: Vec<_> = folded
        .iter()
        .map(|row| {
            (
                row.id.as_str(),
                row.heading.as_str(),
                row.collapsed,
                row.hidden,
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            // A category's first channel goes, and its heading stays alone.
            ("", "TEXT", true, true),
            ("b", "", true, false),
            ("c", "", true, false),
            ("", "VOICE", true, true),
            // The voice channel Talk is in stays.
            ("f", "", true, false),
            ("g", "OTHER", false, false),
            ("h", "", false, false),
        ]
    );
}

#[test]
fn clicking_a_category_collapses_it() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    bind_callbacks(&window, &adapter);
    window.invoke_explore_preview();
    let server = adapter.borrow().app.state.servers[0].id.clone();
    window.invoke_navigate_server(server.into());
    let heading = window
        .get_rooms()
        .iter()
        .map(|row| row.heading)
        .find(|heading| !heading.is_empty())
        .expect("the preview server has categories");
    assert!(!window.get_rooms().iter().any(|row| row.collapsed));
    // Nothing the app keeps changes, only how the sidebar shows it.
    window.invoke_toggle_category(heading.clone());
    assert!(window.get_rooms().iter().any(|row| row.collapsed));
    window.invoke_toggle_category(heading);
    assert!(!window.get_rooms().iter().any(|row| row.collapsed));
}

#[test]
fn opening_at_the_new_divider_puts_it_at_the_top_and_the_bar_jumps_back() {
    i_slint_backend_testing::init_no_event_loop();
    let window = TalkWindow::new().unwrap();
    window.set_workspace(true);
    window.set_room_selected(true);
    window.set_room_ready(true);
    window.set_history_start(true);
    let rows = || -> Vec<MessageRow> {
        (0..100)
            .map(|index| {
                let mut row = message_row_plain(&ChatMessage {
                    id: format!("m{index}"),
                    item: format!("i{index}"),
                    author: "Bob".into(),
                    body: format!("Message {index}"),
                    formatted: true,
                    ..Default::default()
                });
                row.first = true;
                row.unread_start = index == 60;
                row
            })
            .collect()
    };
    window.show().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1120., 760.));
    let settle = || {
        for _ in 0..10 {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(1100., 740.),
                });
        }
    };
    settle();
    // The jump comes with the rows, before the list has measured any.
    window.set_messages(ModelRc::new(VecModel::from(rows())));
    window.set_unread_index(60);
    window.set_unread_label("40 new messages".into());
    messages::show_message(&window, 60, "i60", false);
    settle();
    assert!(
        (59..=60).contains(&window.get_top_index()),
        "the divider is at the top, not row {}",
        window.get_top_index()
    );
    assert!(!window.get_unread_above() && !window.get_unread_below());
    assert!(!window.invoke_timeline_at_end());

    // Scrolled on down, the divider is above, and the bar takes the view back.
    for _ in 0..3 {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(700., 380.),
                delta_x: 0.,
                delta_y: -400.,
            });
        settle();
    }
    assert!(window.get_top_index() > 60);
    assert!(window.get_unread_above());
    messages::show_message(&window, 60, "i60", false);
    settle();
    assert!((59..=60).contains(&window.get_top_index()));
    assert!(!window.get_unread_above());
    // Scrolled up past it, it's below.
    for _ in 0..4 {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(700., 380.),
                delta_x: 0.,
                delta_y: 400.,
            });
        settle();
    }
    assert!(window.get_unread_below());
}

#[test]
fn rows_mark_mentions_and_the_new_bar_counts_what_others_sent() {
    let message = |own: bool, mentions_me: bool| ChatMessage {
        own,
        mentions_me,
        time: "14:05".into(),
        // Today, so the bar leaves the date out.
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        ..Default::default()
    };
    assert!(message_row_plain(&message(false, true)).mentions_me);
    assert!(!message_row_plain(&message(false, false)).mentions_me);
    let unread = [
        message(false, true),
        message(true, false),
        message(false, false),
    ];
    assert_eq!(
        refresh::unread_label(&unread),
        "2 new messages since 14:05",
        "Alice's own reply isn't new to her"
    );
    assert_eq!(
        refresh::unread_label(&unread[..1]),
        "1 new message since 14:05"
    );
    let older = ChatMessage {
        timestamp: 1_791_329_400,
        ..message(false, false)
    };
    assert!(
        refresh::unread_label(&[older]).starts_with("1 new message since 14:05 on "),
        "with the date when it wasn't today"
    );
}

#[test]
fn virtualized_lists_request_only_nearby_avatars() {
    i_slint_backend_testing::init_no_event_loop();
    let window = TalkWindow::new().unwrap();
    window.set_workspace(true);
    window.set_room_selected(true);
    window.set_room_ready(true);
    window.set_members_available(true);
    let requested = Rc::new(RefCell::new(HashSet::new()));
    let requests = requested.clone();
    window
        .global::<PictureStore>()
        .on_picture(|_, _| slint::Image::default());
    window.global::<PictureStore>().on_request(move |url| {
        requests.borrow_mut().insert(url.to_string());
    });
    window.set_servers(ModelRc::new(VecModel::from(
        (0..500)
            .map(|index| ServerRow {
                id: index.to_string().into(),
                name: format!("Server {index}").into(),
                picture_url: format!("server/{index}").into(),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    )));
    window.set_roster(ModelRc::new(VecModel::from(
        (0..500)
            .map(|index| MemberRow {
                id: index.to_string().into(),
                name: format!("Member {index}").into(),
                picture_url: format!("member/{index}").into(),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    )));
    window.show().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1120., 760.));
    let settle = || {
        for _ in 0..10 {
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(50));
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(1100., 740.),
                });
        }
    };
    settle();
    let first = requested.borrow().clone();
    let servers = first
        .iter()
        .filter(|url| url.starts_with("server/"))
        .count();
    let members = first
        .iter()
        .filter(|url| url.starts_with("member/"))
        .count();
    assert!(
        (1..30).contains(&servers),
        "requested {servers} of 500 server icons"
    );
    assert!(
        (1..35).contains(&members),
        "requested {members} of 500 member avatars"
    );
    window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(1000., 380.),
            delta_x: 0.,
            delta_y: -600.,
        });
    settle();
    assert!(
        requested
            .borrow()
            .iter()
            .any(|url| !first.contains(url) && url.starts_with("member/"))
    );
    assert!(
        requested.borrow().len() < 100,
        "scrolling does not instantiate the whole list"
    );
    window.hide().unwrap();
}

#[test]
fn replacing_a_temporary_item_reports_the_view_without_changing_row_count() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let window = TalkWindow::new().unwrap();
    let adapter = Adapter::new(
        Arc::new(BackgroundRuntime::new().unwrap()),
        directory.path().into(),
        &window,
    )
    .unwrap();
    adapter.borrow_mut().action(&window, Action::Preview);
    let reported = Rc::new(Cell::new(0));
    let reports = reported.clone();
    window.on_media_viewport(move || reports.set(reports.get() + 1));
    let count = window.get_messages().row_count();
    let mut state = adapter.borrow_mut();
    let room = state.app.state.selected.clone().unwrap();
    let mut messages = (*state.app.state.messages).clone();
    messages[0].item = "e:$confirmed".into();
    state.apply(Event::Timeline { room, messages });
    state.refresh(&window);
    assert_eq!(window.get_messages().row_count(), count);
    assert_eq!(
        window.get_messages().row_data(0).unwrap().item,
        "e:$confirmed"
    );
    assert!(
        reported.get() > 0,
        "the backend receives the new item ids without scrolling"
    );
}

#[test]
fn a_folder_on_the_rail_holds_its_servers_until_opened() {
    let folder = talk_core::chat::ServerFolder {
        id: "discord-folder:7".into(),
        name: None,
        color: Some(0xff0000),
    };
    let server =
        |id: &str, folder: Option<&talk_core::chat::ServerFolder>| talk_core::chat::ChatServer {
            id: id.into(),
            name: id.into(),
            channels: vec![],
            discord: true,
            muted: false,
            unavailable: false,
            icon: Some(format!("{id}.png")),
            folder: folder.cloned(),
        };
    let mut state = AppState::default();
    state.servers = vec![
        server("Alpha", None),
        server("Bravo", Some(&folder)),
        server("Charlie", Some(&folder)),
        server("Delta", None),
    ];
    state.navigation = Navigation::Server("Charlie".into());
    let shown = |open: &[&str]| {
        let open = open.iter().map(|&id| id.to_owned()).collect();
        refresh::rail_rows(&state, &HashMap::new(), &open)
    };
    let rows = |rows: &[ServerRow]| -> Vec<(String, bool, bool, bool, bool)> {
        rows.iter()
            .map(|row| {
                (
                    row.id.to_string(),
                    row.folder,
                    row.nested,
                    row.last,
                    row.selected,
                )
            })
            .collect()
    };
    let closed = shown(&[]);
    assert_eq!(
        rows(&closed),
        [
            ("Alpha".into(), false, false, false, false),
            // Closed, it shows that one of its servers is selected.
            ("discord-folder:7".into(), true, false, false, true),
            ("Delta".into(), false, false, false, false),
        ]
    );
    // Without a name, it's named by its servers.
    assert_eq!(closed[1].name, "Bravo, Charlie");
    assert_eq!(closed[1].tint, slint::Color::from_rgb_u8(0xff, 0, 0));
    let preview: Vec<_> = closed[1]
        .preview
        .iter()
        .map(|server| server.picture_url.to_string())
        .collect();
    assert_eq!(preview, ["Bravo.png", "Charlie.png"]);

    let open = shown(&["discord-folder:7"]);
    assert_eq!(
        rows(&open),
        [
            ("Alpha".into(), false, false, false, false),
            ("discord-folder:7".into(), true, false, false, true),
            ("Bravo".into(), false, true, false, false),
            ("Charlie".into(), false, true, true, true),
            ("Delta".into(), false, false, false, false),
        ]
    );
    assert!(open[1].open && open[3].tint == open[1].tint);
}
