//! Messages as the timeline shows them: their rows, Markdown, embeds, and
//! how they group by day and author.

use super::*;

/// A message continues the previous one's group when the same person sent it
/// within seven minutes, without replying to something else.
pub(super) fn continues(before: &ChatMessage, message: &ChatMessage) -> bool {
    let sender = |message: &ChatMessage| {
        if message.sender.is_empty() {
            message.author.clone()
        } else {
            message.sender.clone()
        }
    };
    let close = before.timestamp == 0
        || message.timestamp == 0
        || message.timestamp.abs_diff(before.timestamp) <= 7 * 60;
    message.reply.is_none() && sender(before) == sender(message) && close
}

/// The local calendar date of a timestamp, such as "3 October 2026"; empty when unknown.
pub(super) fn day(timestamp: u64) -> String {
    talk_core::clock::day(timestamp)
}

/// Markdown as styled text, with web addresses linked. Slint's Markdown
/// subset rejects some syntax, such as headings; those show as typed,
/// keeping their links.
pub(super) fn markdown(body: &str) -> slint::StyledText {
    slint::StyledText::from_markdown(&links::linkify(&keep_blank_lines(body)))
        .ok()
        .or_else(|| plain_with_links(body))
        .unwrap_or_else(|| slint::StyledText::from_plain_text(body))
}

/// `text` with each blank line between its lines holding a no-break space.
/// Discord shows every line break as typed, where Markdown runs blank lines
/// together into one paragraph break, which Slint draws as a plain line
/// break; a line with a no-break space isn't blank to Markdown, so it shows
/// as an empty line. Fenced code keeps its lines as they are.
pub(super) fn keep_blank_lines(text: &str) -> std::borrow::Cow<'_, str> {
    let blank = |line: &str| line.trim().is_empty();
    let lines: Vec<&str> = text.split('\n').collect();
    let (Some(first), Some(last)) = (
        lines.iter().position(|line| !blank(line)),
        lines.iter().rposition(|line| !blank(line)),
    ) else {
        return text.into();
    };
    if !lines[first..last].iter().any(|line| blank(line)) {
        return text.into();
    }
    let mut fenced = false;
    let kept: Vec<&str> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
            }
            if !fenced && index > first && index < last && blank(line) {
                "\u{a0}"
            } else {
                line
            }
        })
        .collect();
    kept.join("\n").into()
}

/// An embed's title in bold, linking to its page when it has one.
pub(super) fn embed_title(title: &str, url: &str) -> slint::StyledText {
    let escaped = escape_markdown(title);
    let markdown = if url.is_empty() {
        format!("**{escaped}**")
    } else {
        format!("**[{escaped}](<{url}>)**")
    };
    slint::StyledText::from_markdown(&markdown)
        .unwrap_or_else(|_| slint::StyledText::from_plain_text(title))
}

/// `text` with Markdown's punctuation escaped by backslashes, so it shows as typed.
pub(super) fn escape_markdown(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let escape = c.is_ascii_punctuation().then_some('\\');
            escape.into_iter().chain([c])
        })
        .collect()
}

/// `text` in bold, as typed.
pub(super) fn bold(text: &str) -> slint::StyledText {
    slint::StyledText::from_markdown(&format!("**{}**", escape_markdown(text)))
        .unwrap_or_else(|_| slint::StyledText::from_plain_text(text))
}

/// An embed's fields in rows: up to three inline fields share one, and any
/// other field takes a row of its own, as in Discord.
pub(super) fn field_rows(fields: &[EmbedField]) -> ModelRc<EmbedFieldRow> {
    let mut rows: Vec<Vec<talk_ui::EmbedField>> = Vec::new();
    let mut open = false;
    for field in fields {
        let cell = talk_ui::EmbedField {
            name: bold(&field.name),
            value: markdown(&field.value),
        };
        match rows.last_mut() {
            Some(row) if open && field.inline && row.len() < 3 => row.push(cell),
            _ => rows.push(vec![cell]),
        }
        open = field.inline;
    }
    ModelRc::new(VecModel::from(
        rows.into_iter()
            .map(|fields| EmbedFieldRow {
                fields: ModelRc::new(VecModel::from(fields)),
            })
            .collect::<Vec<_>>(),
    ))
}

/// Plain text with its web addresses linked; none when it has none.
pub(super) fn plain_with_links(body: &str) -> Option<slint::StyledText> {
    links::plain_with_links(body)
        .and_then(|markdown| slint::StyledText::from_markdown(&markdown).ok())
}

/// Scrolls the timeline to the message at `index` and highlights it for a moment. The
/// list builds rows as they come into view, so the row settles its place a few times
/// as those around it measure.
pub(super) fn show_message(window: &TalkWindow, index: usize, item: &str) {
    // At once, so the list stops following the newest message before anything
    // else scrolls it.
    window.invoke_show_message(i32::try_from(index).unwrap_or(i32::MAX), item.into());
    for delay in [0, 30, 100, 250] {
        let weak = window.as_weak();
        slint::Timer::single_shot(Duration::from_millis(delay), move || {
            if let Some(window) = weak.upgrade() {
                window.invoke_settle_jump();
            }
        });
    }
    let weak = window.as_weak();
    let shown: SharedString = item.into();
    slint::Timer::single_shot(Duration::from_millis(2500), move || {
        if let Some(window) = weak.upgrade()
            && window.get_jump_item() == shown
        {
            window.set_jump_item(SharedString::default());
        }
    });
}

/// A message as the timeline shows it. `revealed` holds the spoilers clicked, as
/// `{item}/{index}` for text and `{item}/a{index}` for attachments.
pub(super) fn message_row(message: &ChatMessage, revealed: &HashSet<String>) -> MessageRow {
    // Plain text shows as typed, its web addresses linked.
    // Mentions link to who they mention, which opens their card.
    // Custom emoji show as their glyphs once they have them.
    let body = custom_emoji::text(&message.body, &message.emoji);
    let jumbo = message.formatted && custom_emoji::only_emoji(&body);
    let styled = if message.formatted {
        let linked = mention::linked(&body, &message.mentions);
        if message.spoilers {
            markdown(&spoiler::mark(&linked, &message.item, |index| {
                revealed.contains(&format!("{}/{index}", message.item))
            }))
        } else {
            markdown(&linked)
        }
    } else {
        plain_with_links(&message.body)
            .unwrap_or_else(|| slint::StyledText::from_plain_text(&message.body))
    };
    // An image with a preview shows in the gallery; any other attachment shows
    // a file card. Embeds' pictures show in their cards.
    // Pictures, and those on their way, which show as skeletons.
    let previews = |placement: Placement| -> Vec<Preview<'_>> {
        message
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, attachment)| attachment.placement == placement)
            .filter(|(_, attachment)| attachment.preview.is_some() || loading(attachment))
            .map(|(index, attachment)| (index, attachment, attachment.preview.as_ref()))
            .collect()
    };
    let covered = |index: usize| !revealed.contains(&format!("{}/a{index}", message.item));
    let (images, gallery_width, gallery_height) = gallery(&previews(Placement::Message), &covered);
    let stickers: Vec<ImageCell> = previews(Placement::Sticker)
        .into_iter()
        .map(|(index, attachment, preview)| {
            let Some(preview) = preview else {
                return ImageCell {
                    index: index as i32,
                    name: attachment.name.clone().into(),
                    width: STICKER_SIZE,
                    height: STICKER_SIZE,
                    loading: true,
                    ..Default::default()
                };
            };
            let (frames, delays) = animation(preview);
            ImageCell {
                index: index as i32,
                name: attachment.name.clone().into(),
                image: picture(preview),
                width: preview.width as i32,
                height: preview.height as i32,
                frames,
                delays,
                ..Default::default()
            }
        })
        .collect();
    let embeds: Vec<EmbedCard> = message
        .embeds
        .iter()
        .enumerate()
        .map(|(index, embed)| {
            let (images, gallery_width, gallery_height) =
                gallery(&previews(Placement::Embed(index)), &covered);
            let thumbnail = previews(Placement::Thumbnail(index)).into_iter().next();
            EmbedCard {
                color: role_color(embed.color),
                has_color: embed.color.is_some(),
                provider: slint::StyledText::from_plain_text(&embed.provider),
                has_provider: !embed.provider.is_empty(),
                author: bold(&embed.author),
                has_author: !embed.author.is_empty(),
                title: embed_title(&embed.title, &embed.url),
                has_title: !embed.title.is_empty(),
                url: embed.url.clone().into(),
                description: markdown(&embed.description),
                has_description: !embed.description.is_empty(),
                field_rows: field_rows(&embed.fields),
                footer: slint::StyledText::from_plain_text(&embed.footer),
                has_footer: !embed.footer.is_empty(),
                thumbnail: thumbnail
                    .and_then(|(_, _, preview)| preview.map(|preview| picture(preview)))
                    .unwrap_or_default(),
                thumbnail_loading: thumbnail.is_some_and(|(_, _, preview)| preview.is_none()),
                thumbnail_index: thumbnail.map_or(-1, |(index, _, _)| index as i32),
                has_thumbnail: thumbnail.is_some(),
                images: ModelRc::new(VecModel::from(images)),
                gallery_width,
                gallery_height,
                video: embed.video,
            }
        })
        .collect();
    let files: Vec<FileCell> = message
        .attachments
        .iter()
        .enumerate()
        .filter(|(_, attachment)| {
            attachment.preview.is_none()
                && !loading(attachment)
                && attachment.placement == Placement::Message
        })
        .map(|(index, attachment)| {
            let size = attachment.size.map(file_size);
            let detail = match (attachment.image, attachment.preview_failed, size) {
                (true, true, Some(size)) => format!("{size} · Preview unavailable"),
                (true, true, None) => "Preview unavailable".into(),
                (_, _, Some(size)) => size,
                _ => String::new(),
            };
            FileCell {
                index: index as i32,
                name: attachment.name.clone().into(),
                detail: detail.into(),
            }
        })
        .collect();
    let (delivery, can_retry, can_cancel) = match message.delivery {
        Delivery::Sent => ("", false, false),
        Delivery::Sending => ("Sending...", false, true),
        Delivery::Waiting => (
            "Not sent yet. Talk sends it when the connection returns.",
            false,
            true,
        ),
        Delivery::Failed => ("Not sent. The server refused this message.", true, true),
    };
    let reply_author = message
        .reply
        .as_ref()
        .map(|reply| reply.author.as_str())
        .unwrap_or_default();
    let row = MessageRow {
        id: message.id.clone().into(),
        item: message.item.clone().into(),
        author: message.author.clone().into(),
        color: role_color(message.color),
        has_color: message.color.is_some(),
        bot: message.bot,
        initial: initial(&message.author),
        tint: tint(&message.author),
        // Set from the picture cache when the timeline is built.
        picture: slint::Image::default(),
        body: message.body.clone().into(),
        time: message.time.clone().into(),
        // The gutter beside a grouped message shows only the clock time.
        short_time: message
            .time
            .split_whitespace()
            .next()
            .filter(|time| time.contains(':'))
            .unwrap_or_default()
            .into(),
        own: message.own,
        // Grouping depends on the neighbouring messages; refresh sets both.
        first: true,
        day: SharedString::default(),
        jumbo,
        styled,
        author_styled: bold(&message.author),
        time_styled: slint::StyledText::from_plain_text(&message.time),
        edited: message.edited,
        deleted: message.deleted,
        has_reply: message.reply.is_some(),
        reply_author: reply_author.into(),
        reply_initial: initial(reply_author),
        reply_tint: tint(reply_author),
        // Set from the picture cache when the timeline is built.
        reply_picture: slint::Image::default(),
        reply_body: message
            .reply
            .as_ref()
            .map(|reply| reply.body.clone())
            .unwrap_or_default()
            .into(),
        reply_item: message
            .reply
            .as_ref()
            .and_then(|reply| reply.item.clone())
            .unwrap_or_default()
            .into(),
        reactions: ModelRc::new(VecModel::from(
            message
                .reactions
                .iter()
                .map(|reaction| ReactionChip {
                    key: reaction.key.clone().into(),
                    label: custom_emoji::text(&reaction.key, &message.emoji).into(),
                    count: i32::try_from(reaction.count).unwrap_or(i32::MAX),
                    mine: reaction.mine,
                })
                .collect::<Vec<_>>(),
        )),
        images: ModelRc::new(VecModel::from(images)),
        gallery_width,
        gallery_height,
        stickers: ModelRc::new(VecModel::from(stickers)),
        files: ModelRc::new(VecModel::from(files)),
        embeds: ModelRc::new(VecModel::from(embeds)),
        hidden_card: SharedString::default(),
        delivery: delivery.into(),
        delivery_failed: message.delivery == Delivery::Failed,
        can_retry,
        can_cancel,
        can_reply: message.can_reply,
        can_edit: message.can_edit,
        can_delete: message.can_delete,
        can_react: message.can_react,
        can_forward: message.can_forward,
    };
    if collapsed(message, revealed) {
        // Discord shows nothing of a blocked message until asked.
        let note = format!(
            "*Blocked message.* [Show it](<{BLOCKED_LINK}{}>)",
            message.item
        );
        return MessageRow {
            author: "Blocked user".into(),
            author_styled: bold("Blocked user"),
            initial: "?".into(),
            tint: Default::default(),
            color: Default::default(),
            has_color: false,
            bot: false,
            body: "Blocked message.".into(),
            styled: slint::StyledText::from_markdown(&note)
                .unwrap_or_else(|_| slint::StyledText::from_plain_text("Blocked message.")),
            has_reply: false,
            reactions: ModelRc::default(),
            images: ModelRc::default(),
            gallery_width: 0,
            gallery_height: 0,
            stickers: ModelRc::default(),
            files: ModelRc::default(),
            embeds: ModelRc::default(),
            can_reply: false,
            can_edit: false,
            can_react: false,
            can_forward: false,
            ..row
        };
    }
    row
}

/// Whether a message shows collapsed, being from someone blocked.
pub(super) fn collapsed(message: &ChatMessage, revealed: &HashSet<String>) -> bool {
    message.blocked && !revealed.contains(&format!("{}/blocked", message.item))
}
