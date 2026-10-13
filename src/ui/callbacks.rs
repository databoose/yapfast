//! What the window's callbacks do: most dispatch an action to the application.

use super::*;

pub(super) fn bind_callbacks(window: &TalkWindow, adapter: &Rc<RefCell<Adapter>>) {
    // UI bindings can request images while refresh borrows the adapter.
    // Deliver their requests in one batch after the current UI work.
    let pending = Rc::new(RefCell::new(HashSet::new()));
    let timer = Rc::new(slint::Timer::default());
    let state = Rc::downgrade(adapter);
    let weak = window.as_weak();
    window
        .global::<PictureStore>()
        .on_picture(|url, _revision| avatar_picture(url.as_str()));
    window.global::<PictureStore>().on_request(move |url| {
        if url.is_empty() {
            return;
        }
        pending.borrow_mut().insert(url.to_string());
        if !timer.running() {
            let (pending, state, weak) = (pending.clone(), state.clone(), weak.clone());
            timer.start(
                slint::TimerMode::SingleShot,
                Duration::from_millis(50),
                move || {
                    let urls = pending.borrow_mut().drain().collect();
                    if let (Some(adapter), Some(window)) = (state.upgrade(), weak.upgrade()) {
                        adapter
                            .borrow_mut()
                            .action(&window, Action::LoadPictures(urls));
                    }
                },
            );
        }
    });
    let timer = Rc::new(slint::Timer::default());
    let state = Rc::downgrade(adapter);
    let weak = window.as_weak();
    window.on_media_viewport(move || {
        if timer.running() {
            return;
        }
        let (state, weak) = (state.clone(), weak.clone());
        timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(50),
            move || {
                let (Some(adapter), Some(window)) = (state.upgrade(), weak.upgrade()) else {
                    return;
                };
                let mut adapter = adapter.borrow_mut();
                let messages = &adapter.app.state.messages;
                let Some(room) = adapter.app.state.selected.clone() else {
                    return;
                };
                if messages.is_empty() || window.get_top_index() < 0 {
                    return;
                }
                const MARGIN: usize = 6;
                let first = (window.get_top_index() as usize).min(messages.len() - 1);
                let last = (window.get_bottom_index().max(window.get_top_index()) as usize)
                    .min(messages.len() - 1);
                let items = messages[first.saturating_sub(MARGIN)
                    ..=last.saturating_add(MARGIN).min(messages.len() - 1)]
                    .iter()
                    .map(|message| message.item.clone())
                    .collect();
                adapter.action(&window, Action::VisibleMessages { room, items });
            },
        );
    });
    let state = Rc::downgrade(adapter);
    let weak = window.as_weak();
    OPEN.with(|open| {
        *open.borrow_mut() = Some(Box::new(move |room| {
            let (Some(adapter), Some(window)) = (state.upgrade(), weak.upgrade()) else {
                return;
            };
            let _ = window.show();
            window
                .window()
                .with_winit_window(|winit| winit.focus_window());
            window.set_settings_open(false);
            adapter.borrow_mut().action(&window, Action::Open(room));
        }));
    });
    macro_rules! bind {
        ($callback:ident, $action:expr) => {{
            let adapter = adapter.clone();
            let weak = window.as_weak();
            window.$callback(move || {
                if let Some(window) = weak.upgrade() {
                    adapter.borrow_mut().action(&window, $action);
                }
            });
        }};
    }
    bind!(on_explore_preview, Action::Preview);
    bind!(on_restore, Action::Restore);
    bind!(on_logout, Action::Logout);
    bind!(on_refresh_trust, Action::RefreshTrust);
    bind!(on_retry_sync, Action::RetrySync);
    bind!(
        on_navigate_dms,
        Action::Navigate(Navigation::DirectMessages)
    );
    {
        let adapter = adapter.clone();
        let weak = window.as_weak();
        window.on_send(move || {
            let Some(window) = weak.upgrade() else { return };
            // Sending goes to the newest message, as Discord does, so the sent one shows.
            if window.get_can_send() {
                window.invoke_scroll_to_latest();
            }
            adapter.borrow_mut().action(&window, Action::Send);
        });
    }
    bind!(on_earlier, Action::Earlier);
    bind!(on_jump_to_unread, Action::JumpToUnread);
    bind!(on_accept_request, Action::AnswerRequest(true));
    bind!(on_enlarge_profile, Action::EnlargeProfile);
    bind!(on_close_profile, Action::CloseProfile);
    bind!(on_ignore_request, Action::AnswerRequest(false));
    bind!(on_leave_voice, Action::Voice(voice::Command::Leave));
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_befriend(move |user| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::Befriend(user.to_string()));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_remove_friend_request(move |user| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::RemoveFriendRequest(user.to_string()));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_start_call(move || {
        if let Some(window) = weak.upgrade() {
            let id = state.borrow().app.state.selected.clone();
            if let Some(id) = id {
                state
                    .borrow_mut()
                    .action(&window, Action::RequestJoinVoice(id));
            }
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_answer_call(move |room, accept| {
        if let Some(window) = weak.upgrade() {
            let room = room.to_string();
            state.borrow_mut().action(
                &window,
                if accept {
                    Action::AcceptCall(room)
                } else {
                    Action::DeclineCall(room)
                },
            );
        }
    });
    bind!(on_confirm_switch, Action::ConfirmVoiceSwitch);
    bind!(on_cancel_switch, Action::CancelVoiceSwitch);
    bind!(on_dismiss_voice_error, Action::DismissVoiceError);
    bind!(on_refresh_devices, Action::Voice(voice::Command::Devices));
    bind!(on_confirm_delete_message, Action::ConfirmDelete);
    bind!(on_cancel_delete, Action::CancelDelete);
    bind!(on_cancel_compose, Action::CancelCompose);
    bind!(on_close_forward, Action::CloseForward);
    bind!(on_dismiss_notice, Action::DismissNotice);
    bind!(on_close_image, Action::CloseImage);
    {
        let adapter = adapter.clone();
        let weak = window.as_weak();
        window.on_view_image(move |item, index| {
            if let Some(window) = weak.upgrade() {
                let action = Action::ViewImage {
                    item: item.into(),
                    index: usize::try_from(index).unwrap_or_default(),
                };
                adapter.borrow_mut().action(&window, action);
            }
        });
    }
    {
        let adapter = adapter.clone();
        let weak = window.as_weak();
        window.on_step_image(move |step| {
            if let Some(window) = weak.upgrade() {
                adapter
                    .borrow_mut()
                    .action(&window, Action::StepImage(step as isize));
            }
        });
    }
    macro_rules! bind_item {
        ($callback:ident, $action:expr) => {{
            let adapter = adapter.clone();
            let weak = window.as_weak();
            window.$callback(move |item| {
                if let Some(window) = weak.upgrade() {
                    adapter.borrow_mut().action(&window, $action(item.into()));
                }
            });
        }};
    }
    bind_item!(on_reply, Action::Reply);
    bind_item!(on_jump_to, Action::Jump);
    {
        let adapter = adapter.clone();
        let weak = window.as_weak();
        window.on_prepare_menu(move |id, person| {
            if let Some(window) = weak.upgrade() {
                let (rows, subrows) = menus::menu(&adapter.borrow(), &id, person);
                window.set_menu_rows(ModelRc::new(VecModel::from(rows)));
                window.set_menu_subrows(ModelRc::new(VecModel::from(subrows)));
            }
        });
    }
    {
        let adapter = adapter.clone();
        let weak = window.as_weak();
        window.on_choose_menu(move |target, person, id| {
            if let Some(window) = weak.upgrade() {
                menus::choose(
                    &mut adapter.borrow_mut(),
                    &window,
                    target.into(),
                    person,
                    &id,
                );
            }
        });
    }
    {
        let adapter = adapter.clone();
        let weak = window.as_weak();
        window.on_show_friend_profile(move |user, large| {
            if let Some(window) = weak.upgrade() {
                let user = user.into();
                adapter
                    .borrow_mut()
                    .action(&window, Action::ShowFriendProfile { user, large });
            }
        });
    }
    {
        let adapter = adapter.clone();
        let weak = window.as_weak();
        window.on_enlarge_panel_profile(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut adapter = adapter.borrow_mut();
            if let Some(room) = adapter.app.state.selected.clone() {
                adapter.action(&window, Action::ShowDmProfile(room));
            }
        });
    }
    bind_item!(
        on_show_profile,
        (|user| Action::ShowProfile { user, large: false })
    );
    bind_item!(on_edit, Action::Edit);
    {
        let adapter = adapter.clone();
        let weak = window.as_weak();
        window.on_edit_last(move || {
            let Some(window) = weak.upgrade() else { return };
            adapter.borrow_mut().action(&window, Action::EditLast);
            // The message's text, with the cursor after it.
            let end = window.get_draft().len();
            window.invoke_place_composer_caret(i32::try_from(end).unwrap_or(i32::MAX));
        });
    }
    bind_item!(on_request_delete, Action::RequestDelete);
    bind_item!(on_retry, Action::Retry);
    bind_item!(on_cancel_send, Action::CancelSend);
    bind_item!(on_start_forward, Action::StartForward);
    bind_item!(on_forward_to, Action::Forward);
    bind!(on_open_new_dm, Action::OpenNewDm);
    bind!(on_open_add_friend, Action::OpenAddFriend);
    bind!(on_open_requests, Action::OpenDmScreen(DmScreen::Requests));
    bind!(on_open_friends, Action::OpenDmScreen(DmScreen::Friends));
    bind_item!(on_message_friend, Action::MessagePerson);
    bind!(on_link_discord, Action::LinkDiscord);
    bind!(on_cancel_discord_link, Action::CancelDiscordLink);
    bind!(on_unlink_discord, Action::UnlinkDiscord);
    bind!(on_close_add_friend, Action::CloseAddFriend);
    bind!(on_close_person, Action::ClosePersonDialog);
    bind_item!(on_answer_person, Action::AnswerPerson);
    bind_item!(on_add_friend, Action::AddFriend);
    bind!(on_open_status, Action::OpenStatusEditor);
    bind_item!(on_set_status, Action::SetStatus);
    bind!(on_close_status, Action::CloseStatusEditor);
    bind!(on_clear_status, Action::ClearCustomStatus);
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_save_status(move || {
        let Some(window) = weak.upgrade() else { return };
        let key = window.get_status_emoji().key.to_string();
        let action = Action::SetCustomStatus {
            emoji: (!key.is_empty()).then_some(key),
            text: window.get_status_text().to_string(),
            clear_after: match window.get_status_clear_after() {
                1 => ClearAfter::Minutes(30),
                2 => ClearAfter::Minutes(60),
                3 => ClearAfter::Minutes(240),
                4 => ClearAfter::Today,
                _ => ClearAfter::Never,
            },
        };
        state.borrow_mut().action(&window, action);
    });
    bind!(on_close_role, Action::CloseRole);
    bind!(on_close_new_dm, Action::CloseNewDm);
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_new_dm_search(move || {
        if let Some(window) = weak.upgrade() {
            let query = window.get_new_dm_query().to_string();
            state
                .borrow_mut()
                .action(&window, Action::NewDmQuery(query));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_start_dm(move |index| {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        let contact = usize::try_from(index).ok().and_then(|index| {
            adapter
                .app
                .state
                .new_dm
                .as_ref()
                .and_then(|new_dm| new_dm.people().nth(index).cloned())
        });
        if let Some(contact) = contact {
            adapter.action(&window, Action::StartDm(contact));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_forward_search(move || {
        if let Some(window) = weak.upgrade() {
            state.borrow_mut().refresh(&window);
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_emoji_search(move |query| {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        let custom = adapter.app.usable_emoji();
        // Opening the picker fetches the custom emoji's pictures.
        if query.is_empty() {
            let pictures = custom
                .iter()
                .flat_map(|set| &set.emoji)
                .map(|emoji| emoji.picture.clone())
                .take(MAX_PICKER_EMOJI)
                .collect();
            adapter.action(&window, Action::LoadPictures(pictures));
        }
        let (rows, sections) = emoji::rows(
            &query,
            &adapter.app.frequent_reactions(16),
            &custom,
            |set| {
                let icon = adapter.app.state.pictures.get(set.icon.as_deref()?)?;
                Some(picture(icon))
            },
        );
        update_model(&adapter.models.emoji_rows, rows);
        // Matches have no sections; the sidebar keeps those it jumps back to.
        if !sections.is_empty() {
            update_model(&adapter.models.emoji_sections, sections);
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_react(move |item, key| {
        if let Some(window) = weak.upgrade() {
            state.borrow_mut().action(
                &window,
                Action::React {
                    item: item.into(),
                    key: key.into(),
                },
            );
        }
    });
    let state = adapter.clone();
    window.on_sidebar_resized(move |width| {
        let mut adapter = state.borrow_mut();
        adapter.layout.sidebar_width = Some(width);
        adapter.layout.write(&adapter.root);
    });
    let state = adapter.clone();
    window.on_member_list_toggled(move |shown| {
        let mut adapter = state.borrow_mut();
        adapter.layout.members_hidden = !shown;
        adapter.layout.write(&adapter.root);
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_profile_panel_toggled(move |shown| {
        let mut adapter = state.borrow_mut();
        adapter.layout.profile_panel_hidden = !shown;
        adapter.layout.write(&adapter.root);
        if let Some(window) = weak.upgrade() {
            adapter.action(&window, Action::ShowProfilePanel(shown));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_show_hidden_channels_toggled(move |shown| {
        let mut adapter = state.borrow_mut();
        adapter.layout.show_hidden_channels = shown;
        adapter.layout.write(&adapter.root);
        if let Some(window) = weak.upgrade() {
            adapter.action(&window, Action::ShowHiddenChannels(shown));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_reveal_attachment(move |item, index| {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        adapter.revealed.insert(format!("{item}/a{index}"));
        adapter.restyle = true;
        adapter.refresh(&window);
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_open_link(move |link| {
        // A blocked message shows when asked for.
        if let Some(item) = link.strip_prefix(BLOCKED_LINK) {
            let Some(window) = weak.upgrade() else { return };
            let mut adapter = state.borrow_mut();
            adapter.revealed.insert(format!("{item}/blocked"));
            adapter.restyle = true;
            adapter.refresh(&window);
            return;
        }
        // A spoiler shows what it hides.
        if let Some((item, index)) = spoiler::target(&link) {
            let Some(window) = weak.upgrade() else { return };
            let mut adapter = state.borrow_mut();
            adapter.revealed.insert(format!("{item}/{index}"));
            adapter.restyle = true;
            adapter.refresh(&window);
            return;
        }
        // A mention opens the person's card, or the role's members, beside the
        // mention, or by the pointer when the link's place isn't known.
        if let Some(target) = mention::target(&link) {
            let Some(window) = weak.upgrade() else { return };
            if let Some(rect) = i_slint_core::talk_selection::clicked_link() {
                window.invoke_anchor_link(
                    rect.origin.x,
                    rect.origin.y,
                    rect.size.width,
                    rect.size.height,
                );
            }
            let action = match target {
                mention::Target::User(user) => {
                    window.set_profile_at_pointer(true);
                    Action::ShowProfile { user, large: false }
                }
                mention::Target::Role(role) => Action::ShowRole(role),
            };
            state.borrow_mut().action(&window, action);
            return;
        }
        // Only web pages open; a message can't start other programs.
        if links::openable(&link)
            && let Err(error) = webbrowser::open(&link)
        {
            tracing::warn!(%error, "could not open a link");
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_preferences_changed(move || {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        let layout = &mut adapter.layout;
        layout.twelve_hour_clock = window.get_twelve_hour_clock();
        layout.text_size = Some(window.get_text_size());
        layout.link_previews = Some(window.get_link_previews());
        layout.unmute_on_join = window.get_unmute_on_join();
        layout.deafen_on_join = window.get_deafen_on_join();
        layout.level_voice = Some(window.get_level_voice());
        talk_core::leveling::set_enabled(window.get_level_voice());
        layout.noise_suppression = chosen(&SUPPRESSORS, window.get_noise_suppression());
        layout.noise_strength = chosen(&STRENGTHS, window.get_noise_strength());
        talk_core::noise::set(layout.noise_suppression);
        talk_core::noise::set_strength(layout.noise_strength);
        layout.reduce_room_echo = window.get_dereverb();
        talk_core::noise::set_dereverb(layout.reduce_room_echo);
        layout.input_sensitivity = Some(window.get_sensitivity());
        talk_core::noise::set_sensitivity(layout.sensitivity() as f32);
        layout.share_typing = Some(window.get_share_typing());
        layout.notifications = Some(window.get_notifications());
        layout.notification_text = Some(window.get_notification_text());
        layout.sound_set = chosen(&SoundSet::ALL, window.get_sound_set());
        layout.sound_volume = Some(window.get_sound_volume().clamp(0, 200) as u8);
        layout.adjust_each_sound = window.get_adjust_each_sound();
        adapter.layout.write(&adapter.root);
        adapter.restyle = true;
        adapter.refresh(&window);
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_each_sound_changed(move |index, percent| {
        let Some((row, &sound)) = usize::try_from(index)
            .ok()
            .and_then(|row| Some((row, Sound::ALL.get(row)?)))
        else {
            return;
        };
        // The sliders show the list again when they next appear.
        if let Some(window) = weak.upgrade() {
            window.get_each_sound().set_row_data(row, percent);
        }
        let mut adapter = state.borrow_mut();
        let percent = percent.clamp(0, 100) as u8;
        if percent == 100 {
            adapter.layout.each_sound.remove(&sound);
        } else {
            adapter.layout.each_sound.insert(sound, percent);
        }
        adapter.layout.write(&adapter.root);
    });
    let state = adapter.clone();
    window.on_play_sound(move |index| {
        if let Some(&sound) = usize::try_from(index)
            .ok()
            .and_then(|index| Sound::ALL.get(index))
        {
            state.borrow().play(sound);
        }
    });
    window.on_volume_of(|person, stream| {
        let kind = if stream {
            talk_core::volume::Kind::Stream
        } else {
            talk_core::volume::Kind::Voice
        };
        (talk_core::volume::get(kind, &person) * 100.0).round() as i32
    });
    let state = adapter.clone();
    window.on_volume_changed(move |person, stream, percent, done| {
        let volume = percent as f32 / 100.0;
        let kind = if stream {
            talk_core::volume::Kind::Stream
        } else {
            talk_core::volume::Kind::Voice
        };
        talk_core::volume::set(kind, &person, volume);
        // The drag's last step is remembered, rather than every one.
        if done {
            let mut adapter = state.borrow_mut();
            let volumes = if stream {
                &mut adapter.layout.stream_volumes
            } else {
                &mut adapter.layout.volumes
            };
            if percent == 100 {
                volumes.remove(person.as_str());
            } else {
                volumes.insert(person.into(), volume);
            }
            adapter.layout.write(&adapter.root);
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_sync_discord_reads_toggled(move |sync| {
        let mut adapter = state.borrow_mut();
        adapter.layout.sync_discord_reads = Some(sync);
        adapter.layout.write(&adapter.root);
        if let Some(window) = weak.upgrade() {
            adapter.action(&window, Action::SyncDiscordReads(sync));
        }
    });
    let state = adapter.clone();
    window.on_attach(move || {
        let weak = Rc::downgrade(&state);
        state.borrow_mut().dialog(async move {
            let files = rfd::AsyncFileDialog::new()
                .set_title("Attach files")
                .pick_files()
                .await;
            if let (Some(files), Some(adapter)) = (files, weak.upgrade()) {
                let window = adapter.borrow().window.upgrade();
                if let Some(window) = window {
                    let paths = files.iter().map(|file| file.path().to_owned()).collect();
                    adapter.borrow_mut().action(&window, Action::Stage(paths));
                }
            }
        });
    });
    let state = adapter.clone();
    window.on_unstage(move |index| {
        let mut adapter = state.borrow_mut();
        if let (Ok(index), Some(window)) = (usize::try_from(index), adapter.window.upgrade()) {
            adapter.action(&window, Action::Unstage(index));
        }
    });
    let state = adapter.clone();
    window.on_toggle_spoiler(move |index| {
        let mut adapter = state.borrow_mut();
        if let (Ok(index), Some(window)) = (usize::try_from(index), adapter.window.upgrade()) {
            adapter.action(&window, Action::ToggleSpoiler(index));
        }
    });
    let state = adapter.clone();
    window.on_paste_into_composer(move || state.borrow_mut().paste());
    let state = adapter.clone();
    window.on_save_attachment(move |item, index| {
        let item = item.to_string();
        let index = usize::try_from(index).unwrap_or_default();
        let name = state
            .borrow()
            .app
            .state
            .message(&item)
            .and_then(|message| message.attachments.get(index))
            .map(|attachment| attachment.name.clone())
            .unwrap_or_default();
        let weak = Rc::downgrade(&state);
        state.borrow_mut().dialog(async move {
            // Only the file name is suggested; a sender cannot choose the folder.
            let name = std::path::Path::new(&name)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "attachment".into());
            let file = rfd::AsyncFileDialog::new()
                .set_title("Save attachment")
                .set_file_name(name)
                .save_file()
                .await;
            if let (Some(file), Some(adapter)) = (file, weak.upgrade()) {
                let window = adapter.borrow().window.upgrade();
                if let Some(window) = window {
                    adapter.borrow_mut().action(
                        &window,
                        Action::SaveAttachment {
                            item,
                            index,
                            path: file.path().to_owned(),
                        },
                    );
                }
            }
        });
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_login(move || {
        if let Some(window) = weak.upgrade() {
            let mut state = state.borrow_mut();
            let busy = state.app.state.busy;
            state.action(
                &window,
                Action::Login {
                    homeserver: window.get_homeserver().into(),
                    username: window.get_username().into(),
                    password: window.get_password().into(),
                },
            );
            if !busy && state.app.state.busy {
                window.set_password(SharedString::default());
            }
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_recover(move || {
        if let Some(window) = weak.upgrade() {
            let mut state = state.borrow_mut();
            let busy = state.app.state.busy;
            state.action(&window, Action::Recover(window.get_recovery().into()));
            if !busy && state.app.state.busy {
                window.set_recovery(SharedString::default());
            }
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_navigate_server(move |id| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::Navigate(Navigation::Server(id.into())));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_toggle_folder(move |id| {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        if !adapter.layout.open_folders.remove(id.as_str()) {
            adapter.layout.open_folders.insert(id.into());
        }
        adapter.layout.write(&adapter.root);
        // What the sidebar was built from leaves the layout out.
        adapter.sidebar = None;
        adapter.show_sidebar(&window);
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_select_room(move |id| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::SelectRoom(id.into()));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_composer_caret(move |text, offset| {
        let Some(window) = weak.upgrade() else { return };
        // Choosing a suggestion moves the cursor while it's being handled.
        let Ok(mut adapter) = state.try_borrow_mut() else {
            return;
        };
        // The cursor moves before the edit is reported; take the text first, so the
        // refresh below doesn't put back the text before it.
        if adapter.app.state.draft() != text.as_str() {
            adapter.action(&window, Action::SetDraft(text.to_string()));
        }
        let offset = usize::try_from(offset).ok();
        adapter.caret = adapter.app.state.selected.clone().zip(offset);
        // The `:name` word, unless its suggestions were closed.
        let word = offset.and_then(|offset| emoji::query(&text, offset));
        if word.is_none() {
            adapter.emoji_dismissed = None;
        }
        let word = word
            .filter(|(at, _)| adapter.emoji_dismissed != Some(*at))
            .map(|(at, name)| (at, name.to_owned()));
        if adapter.emoji_word != word {
            adapter.emoji_word = word;
            window.set_emoji_suggestion_index(0);
            adapter.show_emoji_suggestions(&window);
            // Custom emoji show their pictures once they arrive.
            let wanted: Vec<String> = adapter
                .app
                .usable_emoji()
                .into_iter()
                .flat_map(|set| set.emoji)
                .filter(|custom| {
                    let key = emoji::custom_key(custom);
                    !adapter.app.state.pictures.contains_key(&custom.picture)
                        && adapter
                            .models
                            .emoji_suggestions
                            .iter()
                            .any(|cell| cell.key == key)
                })
                .map(|custom| custom.picture)
                .collect();
            if !wanted.is_empty() {
                adapter.action(&window, Action::LoadPictures(wanted));
            }
        }
        let found = offset.and_then(|offset| mention::query(&text, offset));
        if found.is_none() {
            adapter.mention_dismissed = None;
        }
        let found = found.filter(|(at, _)| adapter.mention_dismissed != Some(*at));
        adapter.mention_at = found.map(|(at, _)| at);
        let query = found.map(|(_, query)| query.to_owned());
        if adapter.app.state.mention_query != query {
            window.set_mention_index(0);
            adapter.action(&window, Action::MentionQuery(query));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_mention_chosen(move |index| {
        let Some(window) = weak.upgrade() else { return };
        let caret = {
            let mut adapter = state.borrow_mut();
            let chosen = usize::try_from(index).ok().and_then(|index| {
                adapter
                    .app
                    .state
                    .mention_suggestions()
                    .into_iter()
                    .nth(index)
            });
            let (Some(chosen), Some(at), Some(query)) = (
                chosen,
                adapter.mention_at,
                adapter.app.state.mention_query.clone(),
            ) else {
                return;
            };
            let text = adapter.app.state.draft().to_owned();
            let end = at + 1 + query.len();
            if text.get(at..end) != Some(format!("@{query}").as_str()) {
                return;
            }
            let inserted = format!("@{} ", chosen.name);
            let text = format!("{}{inserted}{}", &text[..at], &text[end..]);
            adapter.mention_at = None;
            adapter.action(&window, Action::MentionQuery(None));
            adapter.action(&window, Action::SetDraft(text));
            at + inserted.len()
        };
        window.invoke_place_composer_caret(i32::try_from(caret).unwrap_or(i32::MAX));
        window.invoke_focus_composer();
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_emoji_suggestion_chosen(move |index| {
        let Some(window) = weak.upgrade() else { return };
        let caret = {
            let mut adapter = state.borrow_mut();
            let chosen = usize::try_from(index)
                .ok()
                .and_then(|index| adapter.models.emoji_suggestions.row_data(index));
            let word = adapter.emoji_word.take();
            adapter.show_emoji_suggestions(&window);
            let (Some(chosen), Some((at, name))) = (chosen, word) else {
                return;
            };
            let text = adapter.app.state.draft().to_owned();
            let end = at + 1 + name.len();
            if text.get(at..end) != Some(format!(":{name}").as_str()) {
                return;
            }
            let inserted = format!("{} ", emoji::composer_text(&chosen.key));
            let text = format!("{}{inserted}{}", &text[..at], &text[end..]);
            adapter.action(&window, Action::SetDraft(text));
            at + inserted.len()
        };
        window.invoke_place_composer_caret(i32::try_from(caret).unwrap_or(i32::MAX));
        window.invoke_focus_composer();
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_emoji_suggestion_dismissed(move || {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        adapter.emoji_dismissed = adapter.emoji_word.take().map(|(at, _)| at);
        adapter.show_emoji_suggestions(&window);
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_insert_emoji(move |key| {
        let Some(window) = weak.upgrade() else { return };
        let caret = {
            let mut adapter = state.borrow_mut();
            let text = adapter.app.state.draft().to_owned();
            // At the cursor this room's composer last had, else at the end.
            let at = adapter
                .caret
                .as_ref()
                .filter(|(room, _)| adapter.app.state.selected.as_ref() == Some(room))
                .map(|&(_, at)| at)
                .filter(|&at| text.is_char_boundary(at))
                .unwrap_or(text.len());
            let inserted = emoji::composer_text(&key);
            let text = format!("{}{inserted}{}", &text[..at], &text[at..]);
            adapter.action(&window, Action::SetDraft(text));
            at + inserted.len()
        };
        window.invoke_place_composer_caret(i32::try_from(caret).unwrap_or(i32::MAX));
        window.invoke_focus_composer();
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_mention_dismissed(move || {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        adapter.mention_dismissed = adapter.mention_at.take();
        adapter.action(&window, Action::MentionQuery(None));
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_draft_edited(move |text| {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        // The caret's move usually took the text already; a second refresh
        // with nothing changed is wasted work on every keystroke.
        if adapter.app.state.draft() != text.as_str() {
            adapter.action(&window, Action::SetDraft(text.into()));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    // The filters only narrow the sidebar's lists.
    window.on_filter_edited(move || {
        if let Some(window) = weak.upgrade() {
            state.borrow_mut().show_sidebar(&window);
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_toggle_category(move |heading| {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        let Navigation::Server(server) = adapter.app.state.navigation.clone() else {
            return;
        };
        let collapsed = adapter.layout.collapsed.entry(server.clone()).or_default();
        if !collapsed.remove(heading.as_str()) {
            collapsed.insert(heading.into());
        }
        if collapsed.is_empty() {
            adapter.layout.collapsed.remove(&server);
        }
        adapter.layout.write(&adapter.root);
        // What the sidebar was built from leaves the layout out.
        adapter.sidebar = None;
        adapter.show_sidebar(&window);
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_friend_filter_edited(move || {
        if let Some(window) = weak.upgrade() {
            state.borrow_mut().show_sidebar(&window);
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_join_voice(move || {
        if let Some(window) = weak.upgrade() {
            let id = state.borrow().app.state.selected.clone();
            if let Some(id) = id {
                state
                    .borrow_mut()
                    .action(&window, Action::RequestJoinVoice(id));
            }
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_toggle_mute(move || {
        if let Some(window) = weak.upgrade() {
            if !state.borrow().app.state.voice.active() {
                state.borrow_mut().toggle_on_join(&window, false);
                return;
            }
            let muted = state.borrow().app.state.voice.muted;
            state
                .borrow_mut()
                .action(&window, Action::Voice(voice::Command::Mute(!muted)));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_toggle_share(move || {
        if let Some(window) = weak.upgrade() {
            let command = {
                let state = state.borrow();
                let voice = &state.app.state.voice;
                if voice.sharing.is_some() {
                    voice::Command::StopLive
                } else {
                    voice::Command::GoLive(voice.quality)
                }
            };
            state.borrow_mut().action(&window, Action::Voice(command));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_go_live(move |resolution, fps| {
        if let Some(window) = weak.upgrade() {
            let resolution = match resolution {
                0 => voice::Resolution::P720,
                2 => voice::Resolution::Source,
                _ => voice::Resolution::P1080,
            };
            let quality = voice::StreamQuality {
                resolution,
                fps: fps.clamp(1, 60) as u32,
            };
            state
                .borrow_mut()
                .action(&window, Action::Voice(voice::Command::GoLive(quality)));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_watch_stream(move |id| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::Voice(voice::Command::Watch(id.into())));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_stop_watching(move || {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::Voice(voice::Command::StopWatching));
        }
    });
    // Frames skip the application state, which would redraw far more than the pictures.
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_stream_tick(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let adapter = state.borrow();
        let voice = &adapter.app.state.voice;
        let image = |width, height, pixels: &[u8]| {
            slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
                pixels, width, height,
            ))
        };
        if let Some(frame) = voice
            .sharing
            .as_ref()
            .and_then(|sharing| sharing.frames.take(image))
        {
            window.set_share_frame(frame);
        }
        if let Some(frame) = voice
            .watching
            .as_ref()
            .and_then(|watching| watching.frames.as_ref())
            .and_then(|frames| frames.take(image))
        {
            window.set_watch_frame(frame);
        }
        let video = adapter.app.state.video.as_ref();
        let taken =
            video.and_then(|video| Some((video, video.frames.as_ref()?.take_timed(image)?)));
        if taken.is_none() && video.is_some_and(|video| !video.playing && !video.loading) {
            // Half a second without a picture: a paused video's ticks stop.
            adapter.video_quiet.set(adapter.video_quiet.get() + 1);
            if adapter.video_quiet.get() > 15 {
                window.set_video_active(false);
            }
        }
        if let Some((video, (frame, time))) = taken {
            adapter.video_quiet.set(0);
            let player = window.global::<VideoPlayer>();
            player.set_frame(frame);
            player.set_has_frame(true);
            // The last picture shows a little before the end.
            let time = match video.duration_ms {
                Some(duration) if video.ended => duration,
                _ => time,
            };
            if let Some(duration) = video.duration_ms.filter(|duration| *duration > 0) {
                player.set_progress((time as f64 / duration as f64).min(1.0) as f32);
            }
            player.set_time(refresh::video_time(time, video.duration_ms).into());
        }
    });
    // The video playing: started from its cell, controlled from its bar.
    let player = window.global::<VideoPlayer>();
    let state = adapter.clone();
    let weak = window.as_weak();
    player.on_play(move |item, index, width, height| {
        if let Some(window) = weak.upgrade() {
            let scale = window.window().scale_factor();
            let pixels = |side: f32| (side * scale).round().max(1.0) as u32;
            let (width, height) = (pixels(width), pixels(height));
            let action = Action::PlayVideo {
                item: item.into(),
                index: index as usize,
                width,
                height,
            };
            let mut adapter = state.borrow_mut();
            adapter.video_cell.set((width, height));
            adapter.action(&window, action);
        }
    });
    // Full screen, its pictures come at the viewer's size; back in its
    // cell, at the cell's.
    let state = adapter.clone();
    let weak = window.as_weak();
    player.on_expand(move |expanded| {
        if let Some(window) = weak.upgrade() {
            window.global::<VideoPlayer>().set_expanded(expanded);
            if !expanded {
                let mut adapter = state.borrow_mut();
                let (width, height) = adapter.video_cell.get();
                adapter.action(&window, Action::Video(VideoCommand::Size { width, height }));
            }
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    player.on_resize(move |width, height| {
        if let Some(window) = weak.upgrade()
            && window.global::<VideoPlayer>().get_expanded()
        {
            let scale = window.window().scale_factor();
            let pixels = |side: f32| (side * scale).round().max(1.0) as u32;
            let size = VideoCommand::Size {
                width: pixels(width),
                height: pixels(height),
            };
            state.borrow_mut().action(&window, Action::Video(size));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    player.on_toggle(move || {
        if let Some(window) = weak.upgrade() {
            let playing = state
                .borrow()
                .app
                .state
                .video
                .as_ref()
                .is_some_and(|video| video.playing);
            let command = if playing {
                VideoCommand::Pause
            } else {
                VideoCommand::Play
            };
            state.borrow_mut().action(&window, Action::Video(command));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    player.on_seek(move |fraction| {
        if let Some(window) = weak.upgrade() {
            let duration = state
                .borrow()
                .app
                .state
                .video
                .as_ref()
                .and_then(|video| video.duration_ms);
            if let Some(duration) = duration {
                let at = (f64::from(fraction.clamp(0.0, 1.0)) * duration as f64) as u64;
                state
                    .borrow_mut()
                    .action(&window, Action::Video(VideoCommand::Seek(at)));
            }
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    player.on_mute(move |muted| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::Video(VideoCommand::Mute(muted)));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    player.on_change_volume(move |volume| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::Video(VideoCommand::Volume(volume)));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_join_unmuted(move || {
        if let Some(window) = weak.upgrade() {
            state.borrow_mut().action(&window, Action::UnmuteOnJoin);
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_join_deafened(move || {
        if let Some(window) = weak.upgrade() {
            state.borrow_mut().action(&window, Action::DeafenOnJoin);
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_toggle_deafen(move || {
        if let Some(window) = weak.upgrade() {
            if !state.borrow().app.state.voice.active() {
                state.borrow_mut().toggle_on_join(&window, true);
                return;
            }
            let deafened = state.borrow().app.state.voice.deafened;
            state
                .borrow_mut()
                .action(&window, Action::Voice(voice::Command::Deafen(!deafened)));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_select_microphone(move |index| {
        if let Some(window) = weak.upgrade() {
            let id = usize::try_from(index).ok().and_then(|index| {
                state
                    .borrow()
                    .app
                    .state
                    .voice
                    .microphones
                    .get(index)
                    .map(|device| device.id.clone())
            });
            if let Some(id) = id {
                let mut adapter = state.borrow_mut();
                if adapter.meter.is_some() {
                    adapter.meter = Some(InputMeter::start(Some(&id)));
                }
                #[cfg(feature = "discord")]
                if adapter.gate_meter.is_some() {
                    adapter.gate_meter =
                        Some(talk_core::discord_audio::GateMeter::start(Some(&id)));
                }
                adapter.action(&window, Action::Voice(voice::Command::Microphone(id)));
            }
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_select_speaker(move |index| {
        if let Some(window) = weak.upgrade() {
            let id = usize::try_from(index).ok().and_then(|index| {
                state
                    .borrow()
                    .app
                    .state
                    .voice
                    .speakers
                    .get(index)
                    .map(|device| device.id.clone())
            });
            if let Some(id) = id {
                state
                    .borrow_mut()
                    .action(&window, Action::Voice(voice::Command::Speaker(id)));
            }
        }
    });
    let state = adapter.clone();
    window.on_metering(move |on| {
        let mut adapter = state.borrow_mut();
        let microphone = adapter.app.state.voice.microphone.clone();
        adapter.meter = on.then(|| InputMeter::start(microphone.as_deref()));
    });
    // The level skips the application state, which would redraw far more than the meter.
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_meter_tick(move || {
        if let (Some(window), Some(meter)) = (weak.upgrade(), &state.borrow().meter) {
            window.set_input_level(meter.level());
            window.set_meter_note(SharedString::from(if !meter::AVAILABLE {
                "The level meter isn't available on this system yet."
            } else if meter.failed() {
                "Talk can't read this microphone. Check that it's connected, or choose another."
            } else {
                ""
            }));
        }
    });
    #[cfg(feature = "discord")]
    {
        let state = adapter.clone();
        window.on_gate_metering(move |on| {
            let mut adapter = state.borrow_mut();
            let microphone = adapter.app.state.voice.microphone.clone();
            adapter.gate_meter =
                on.then(|| talk_core::discord_audio::GateMeter::start(microphone.as_deref()));
        });
        let state = adapter.clone();
        let weak = window.as_weak();
        window.on_gate_meter_tick(move || {
            if let (Some(window), Some(meter)) = (weak.upgrade(), &state.borrow().gate_meter) {
                window.set_gate_level(meter.level().max(-100.0));
                window.set_gate_sending(meter.sending());
                window.set_gate_meter_note(SharedString::from(
                    if !talk_core::discord_audio::AVAILABLE {
                        "Discord voice and its level work on Linux for now."
                    } else if meter.failed() {
                        "Talk can't read this microphone. Check that it's connected, or choose another."
                    } else {
                        ""
                    },
                ));
            }
        });
    }
}
