//! Acting on messages: reactions, deleting, forwarding, retrying, attachments and the image viewer, and loading history.

use super::*;

impl Application {
    pub(super) fn react(&mut self, item: String, key: String) -> Vec<Effect> {
        if !self.state.can_write()
            || !self
                .state
                .message(&item)
                .is_some_and(|message| message.can_react)
        {
            return vec![];
        }
        let room = self.state.selected.clone().expect("selected room exists");
        let adding = !self.state.message(&item).is_some_and(|message| {
            message
                .reactions
                .iter()
                .any(|reaction| reaction.key == key && reaction.mine)
        });
        if adding {
            *self.reaction_uses.entry(key.clone()).or_default() += 1;
        }
        if self.state.preview {
            self.update_sample(&room, &item, |message| {
                if let Some(index) = message
                    .reactions
                    .iter()
                    .position(|reaction| reaction.key == key)
                {
                    let reaction = &mut message.reactions[index];
                    reaction.mine = !reaction.mine;
                    reaction.count = if reaction.mine {
                        reaction.count + 1
                    } else {
                        reaction.count.saturating_sub(1)
                    };
                    if reaction.count == 0 {
                        message.reactions.remove(index);
                    }
                } else {
                    message.reactions.push(Reaction {
                        key,
                        count: 1,
                        mine: true,
                    });
                }
            });
            vec![]
        } else {
            vec![Effect::Backend(Command::React { room, item, key })]
        }
    }

    pub(super) fn request_delete(&mut self, item: String) -> Vec<Effect> {
        if self
            .state
            .message(&item)
            .is_some_and(|message| message.can_delete)
        {
            self.state.confirm_delete = Some(item);
        }
        vec![]
    }

    pub(super) fn start_forward(&mut self, item: String) -> Vec<Effect> {
        if self.state.preview || self.state.trusted {
            self.state.forwarding = self
                .state
                .message(&item)
                .filter(|message| message.can_forward)
                .map(|_| Forwarding { item, sent: vec![] });
        }
        vec![]
    }

    pub(super) fn confirm_delete(&mut self) -> Vec<Effect> {
        let Some(item) = self.state.confirm_delete.take() else {
            return vec![];
        };
        let Some(room) = self.state.selected.clone() else {
            return vec![];
        };
        if self
            .state
            .compose
            .get(&room)
            .is_some_and(|compose| match compose {
                Compose::Reply { item: target, .. } | Compose::Edit { item: target, .. } => {
                    target == &item
                }
            })
        {
            self.finish_compose(&room);
        }
        if self.state.preview {
            self.update_sample(&room, &item, |message| {
                message.body = "Message deleted".into();
                message.deleted = true;
                message.reactions.clear();
                message.can_reply = false;
                message.can_edit = false;
                message.can_delete = false;
                message.can_react = false;
                message.can_forward = false;
            });
            vec![]
        } else {
            vec![Effect::Backend(Command::Delete { room, item })]
        }
    }

    pub(super) fn save_attachment(
        &mut self,
        item: String,
        index: usize,
        path: PathBuf,
    ) -> Vec<Effect> {
        if self.state.preview
            || !self
                .state
                .message(&item)
                .is_some_and(|message| index < message.attachments.len())
        {
            return vec![];
        }
        let room = self.state.selected.clone().expect("selected room exists");
        vec![Effect::Backend(Command::SaveAttachment {
            room,
            item,
            index,
            path,
        })]
    }

    pub(super) fn view_image(&mut self, item: String, index: usize) -> Vec<Effect> {
        if !self
            .state
            .message(&item)
            .and_then(|message| message.attachments.get(index))
            .is_some_and(|attachment| attachment.image)
        {
            return vec![];
        }
        self.state.viewer = Some(Viewer {
            item,
            index,
            pictures: HashMap::new(),
        });
        self.load_image()
    }

    pub(super) fn step_image(&mut self, step: isize) -> Vec<Effect> {
        let Some(viewer) = &self.state.viewer else {
            return vec![];
        };
        let images: Vec<usize> = self
            .state
            .message(&viewer.item)
            .into_iter()
            .flat_map(|message| message.attachments.iter().enumerate())
            .filter(|(_, attachment)| attachment.image)
            .map(|(index, _)| index)
            .collect();
        let Some(at) = images.iter().position(|index| *index == viewer.index) else {
            return vec![];
        };
        let next = images[(at as isize + step).rem_euclid(images.len() as isize) as usize];
        if let Some(viewer) = &mut self.state.viewer {
            viewer.index = next;
        }
        self.load_image()
    }

    pub(super) fn earlier(&mut self) -> Vec<Effect> {
        if self.state.preview
            || self.state.busy
            || !self.state.room_ready
            || self.state.loading_history
            || self.state.history_start
            || self.state.selected_room().is_none_or(|room| room.voice)
        {
            return vec![];
        }
        self.state.loading_history = true;
        self.state.history_failed = false;
        vec![Effect::Backend(Command::Earlier(
            self.state.selected.clone().expect("selected room exists"),
        ))]
    }

    pub(super) fn forward(&mut self, target: String) -> Vec<Effect> {
        let (Some(forwarding), Some(source)) =
            (self.state.forwarding.clone(), self.state.selected.clone())
        else {
            return vec![];
        };
        let Some(message) = self.state.message(&forwarding.item).cloned() else {
            return vec![];
        };
        if forwarding.sent.contains(&target)
            || !self
                .state
                .rooms
                .iter()
                .any(|room| room.id == target && self.state.can_forward_to(room))
        {
            return vec![];
        }
        if let Some(forwarding) = &mut self.state.forwarding {
            forwarding.sent.push(target.clone());
        }
        if self.state.preview {
            let item = uuid::Uuid::new_v4().to_string();
            let messages = self.samples.entry(target.clone()).or_default();
            messages.push(ChatMessage {
                id: item.clone(),
                item,
                author: "You".into(),
                sender: "You".into(),
                time: "Local preview".into(),
                own: true,
                body: message.body,
                formatted: message.formatted,
                attachments: message.attachments,
                ..sample_actions()
            });
            if target == source {
                self.state.messages = Arc::new(messages.clone());
            }
            return vec![];
        }
        vec![Effect::Backend(Command::Forward {
            room: source,
            item: forwarding.item,
            target,
        })]
    }

    pub(super) fn queue_action(
        &mut self,
        item: String,
        command: impl FnOnce(String, String) -> Command,
    ) -> Vec<Effect> {
        if self.state.preview
            || !self
                .state
                .message(&item)
                .is_some_and(|message| message.delivery != chat::Delivery::Sent)
        {
            return vec![];
        }
        let room = self.state.selected.clone().expect("selected room exists");
        vec![Effect::Backend(command(room, item))]
    }

    pub(super) fn update_sample(
        &mut self,
        room: &str,
        item: &str,
        change: impl FnOnce(&mut ChatMessage),
    ) {
        if let Some(messages) = self.samples.get_mut(room)
            && let Some(message) = messages.iter_mut().find(|message| message.item == item)
        {
            change(message);
            self.state.messages = Arc::new(messages.clone());
        }
    }

    /// Downloads the viewer's image at viewing size, unless it has it or is
    /// getting it. The preview shows until it arrives.
    pub(super) fn load_image(&mut self) -> Vec<Effect> {
        let (Some(viewer), Some(room)) = (&mut self.state.viewer, &self.state.selected) else {
            return vec![];
        };
        if self.state.preview || viewer.pictures.contains_key(&viewer.index) {
            return vec![];
        }
        viewer.pictures.insert(viewer.index, None);
        vec![Effect::Backend(Command::ViewImage {
            room: room.clone(),
            item: viewer.item.clone(),
            index: viewer.index,
        })]
    }

    /// Brings the message being sought into view once the timeline has it,
    /// else loads another page of history, up to [`MAX_SEEK_PAGES`].
    pub(super) fn seek(&mut self) -> Vec<Effect> {
        let Some(seeking) = self.seeking.take() else {
            return vec![];
        };
        if self
            .state
            .messages
            .iter()
            .any(|message| message.item == seeking.item)
        {
            if seeking.unread {
                self.state.unread_jump += 1;
            } else {
                let count = self.state.jump.as_ref().map_or(0, |(_, count)| count + 1);
                self.state.jump = Some((seeking.item, count));
            }
            return vec![];
        }
        if seeking.pages >= MAX_SEEK_PAGES || self.state.history_start {
            self.state.notice = Some(
                if seeking.unread {
                    "Talk couldn't find the first unread message in this conversation's recent history."
                } else {
                    "Talk couldn't find the original message in this conversation's recent history."
                }
                .into(),
            );
            return vec![];
        }
        let effects = self.dispatch_action(Action::Earlier);
        if self.state.loading_history {
            self.seeking = Some(Seeking {
                pages: seeking.pages + 1,
                ..seeking
            });
        }
        effects
    }

    /// Puts the "New" divider above the first message after the one last
    /// read when the room was opened, once the timeline has it. Discord's
    /// messages are ordered by their snowflakes, so the divider goes in even
    /// when the last read message is gone; earlier history loads until a read
    /// message is there too, up to [`MAX_UNREAD_PAGES`]. A Matrix room's
    /// divider waits for the message its read receipt is on. The timeline
    /// then shows the divider at its top, once.
    pub(super) fn place_unread(&mut self) -> Vec<Effect> {
        let Some((marker, pages)) = self.read_marker.take() else {
            return vec![];
        };
        let messages = Arc::clone(&self.state.messages);
        if !self.state.room_ready || messages.is_empty() {
            self.read_marker = Some((marker, pages));
            return vec![];
        }
        let unread = |message: &ChatMessage| !message.own;
        let first = match discord::item_order(&marker) {
            Some(read) => {
                let after = |message: &ChatMessage| {
                    discord::item_order(&message.item).is_some_and(|order| order > read)
                };
                let reached = messages.iter().any(|message| {
                    discord::item_order(&message.item).is_some_and(|order| order <= read)
                });
                if !reached && !self.state.history_start && pages < MAX_UNREAD_PAGES {
                    // A page already on its way, such as one scrolling asked
                    // for, isn't counted.
                    let effects = self.earlier();
                    self.read_marker = Some((marker, pages + u32::from(!effects.is_empty())));
                    return effects;
                }
                messages
                    .iter()
                    .position(|message| after(message) && unread(message))
            }
            None => messages
                .iter()
                .position(|message| message.item == marker)
                .and_then(|at| {
                    let next = messages[at + 1..].iter().position(unread)?;
                    Some(at + 1 + next)
                }),
        };
        match first {
            Some(first) => {
                self.state.first_unread = Some(messages[first].item.clone());
                self.state.unread_jump += 1;
            }
            // Not loaded yet, or nothing is unread after all; newer messages
            // may still come with the rest of the timeline.
            None => self.read_marker = Some((marker, pages)),
        }
        vec![]
    }
}
