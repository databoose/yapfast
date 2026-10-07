//! Discord's embeds: the link previews Discord makes from a page's Open Graph
//! tags, and the cards bots post. Their pictures come through Discord's media
//! proxy, so showing them never contacts the linked site. See
//! <https://docs.discord.food/resources/message#embed-object>.

use serde_json::Value;

use crate::message::{
    Attachment, PREVIEW_HEIGHT, PREVIEW_WIDTH, Shown, VIEW_HEIGHT, VIEW_WIDTH, date, iso_ms,
    scaled_url,
};

/// The size thumbnails beside an embed's text are downloaded at: twice what
/// they show at, for high-DPI displays.
const THUMBNAIL_SIZE: u64 = 160;

/// An embed as a card. Its pictures are among the message's attachments,
/// placed with [`Shown::Embed`] and [`Shown::Thumbnail`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Embed {
    /// The RGB colour of the bar down its side.
    pub color: Option<u32>,
    /// The site, such as "YouTube".
    pub provider: Option<String>,
    pub author: Option<String>,
    pub title: Option<String>,
    /// Where the title leads.
    pub url: Option<String>,
    /// Discord Markdown with mentions still as `<@id>`.
    pub description: Option<String>,
    pub fields: Vec<Field>,
    /// The footer, with the embed's date after it.
    pub footer: Option<String>,
    /// A video's: its large picture is the video's poster, which opens the
    /// video at `url` rather than in the image viewer.
    pub video: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    /// Discord Markdown.
    pub value: String,
    /// Shares a row with the inline fields beside it.
    pub inline: bool,
}

/// A text field that may be missing, null or blank.
fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
}

/// A picture of an embed as an attachment, when Discord's proxy has it.
fn picture(value: &Value, shown: Shown, thumbnail: bool) -> Option<Attachment> {
    let proxy = value["proxy_url"].as_str()?;
    let side = |name: &str| u32::try_from(value[name].as_u64()?).ok();
    let dimensions = side("width").zip(side("height"));
    let (width, height) = if thumbnail {
        (THUMBNAIL_SIZE, THUMBNAIL_SIZE)
    } else {
        (PREVIEW_WIDTH, PREVIEW_HEIGHT)
    };
    let preview_url = scaled_url(value, width, height)?;
    let source = value["url"].as_str().unwrap_or(proxy);
    let name = source
        .split(['?', '#'])
        .next()
        .and_then(|path| path.rsplit('/').next())
        .filter(|name| !name.is_empty())
        .unwrap_or("image");
    Some(Attachment {
        name: name.to_owned(),
        size: None,
        url: proxy.to_owned(),
        image: !matches!(shown, Shown::Poster(_)),
        dimensions,
        preview_url: Some(preview_url),
        view_url: scaled_url(value, VIEW_WIDTH, VIEW_HEIGHT),
        shown,
        spoiler: false,
    })
}

/// A message's embeds as cards, and their pictures as attachments to add
/// after the message's own. An image or GIF linked on its own shows like an
/// attached one, without a card; `hides` is that link, which Discord leaves
/// out of the message's text when it's all the text there is.
pub struct Parsed {
    pub embeds: Vec<Embed>,
    pub pictures: Vec<Attachment>,
    pub hides: Vec<String>,
}

pub fn parse(embeds: &Value) -> Parsed {
    let mut parsed = Parsed {
        embeds: Vec::new(),
        pictures: Vec::new(),
        hides: Vec::new(),
    };
    for embed in embeds.as_array().into_iter().flatten() {
        let kind = embed["type"].as_str().unwrap_or("rich");
        let url = text(&embed["url"]);
        match kind {
            // A linked image or GIF: Discord puts the picture in the thumbnail.
            "image" | "gifv" => {
                let source = if embed["thumbnail"].is_object() {
                    &embed["thumbnail"]
                } else {
                    &embed["image"]
                };
                if let Some(picture) = picture(source, Shown::Message, false) {
                    parsed.pictures.push(picture);
                    parsed.hides.extend(url);
                }
                continue;
            }
            "rich" | "article" | "link" | "video" => {}
            // Automod notices, poll results and the like.
            _ => continue,
        }
        // A post with several pictures comes as one embed per picture, the
        // later ones only a picture under the same address; they share a card.
        let joins = parsed.embeds.last().filter(|last| {
            url.is_some()
                && last.url == url
                && text(&embed["title"]).is_none()
                && text(&embed["description"]).is_none()
        });
        if joins.is_some() {
            let index = parsed.embeds.len() - 1;
            parsed
                .pictures
                .extend(picture(&embed["image"], Shown::Embed(index), false));
            continue;
        }
        let index = parsed.embeds.len();
        let video = kind == "video" && embed["video"].is_object();
        // Articles and videos show their thumbnail large, as Discord does.
        let large_thumbnail = matches!(kind, "article" | "video");
        if embed["image"].is_object() {
            parsed
                .pictures
                .extend(picture(&embed["image"], Shown::Embed(index), false));
        } else if large_thumbnail {
            let shown = if video {
                Shown::Poster(index)
            } else {
                Shown::Embed(index)
            };
            parsed
                .pictures
                .extend(picture(&embed["thumbnail"], shown, false));
        }
        if !large_thumbnail || embed["image"].is_object() {
            parsed
                .pictures
                .extend(picture(&embed["thumbnail"], Shown::Thumbnail(index), true));
        }
        let footer = text(&embed["footer"]["text"]);
        let when = embed["timestamp"].as_str().and_then(iso_ms).map(date);
        parsed.embeds.push(Embed {
            color: embed["color"]
                .as_u64()
                .and_then(|color| u32::try_from(color).ok()),
            provider: text(&embed["provider"]["name"]),
            author: text(&embed["author"]["name"]),
            title: text(&embed["title"]),
            url,
            description: text(&embed["description"]),
            fields: embed["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|field| {
                    Some(Field {
                        name: field["name"].as_str()?.to_owned(),
                        value: field["value"].as_str()?.to_owned(),
                        inline: field["inline"].as_bool().unwrap_or(false),
                    })
                })
                .collect(),
            footer: match (footer, when) {
                (Some(footer), Some(when)) => Some(format!("{footer} • {when}")),
                (footer, when) => footer.or(when),
            },
            video,
        });
    }
    parsed
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn proxied(name: &str, width: u64, height: u64) -> Value {
        json!({
            "url": format!("https://example.com/{name}"),
            "proxy_url": format!("https://images-ext-1.discordapp.net/external/x/https/example.com/{name}"),
            "width": width,
            "height": height,
        })
    }

    #[test]
    fn link_previews_become_cards_with_their_pictures() {
        let parsed = parse(&json!([
            {"type": "article", "url": "https://example.com/news", "title": "News",
             "description": "What happened", "color": 0x123456,
             "provider": {"name": "Example"}, "thumbnail": proxied("news.jpg", 1200, 630)},
            {"type": "link", "url": "https://example.com/about", "title": "About",
             "thumbnail": proxied("logo.png", 64, 64)},
            {"type": "video", "url": "https://youtube.com/watch?v=1", "title": "A video",
             "provider": {"name": "YouTube"}, "author": {"name": "A channel"},
             "thumbnail": proxied("poster.jpg", 1280, 720),
             "video": {"url": "https://youtube.com/embed/1", "width": 1280, "height": 720}},
            {"type": "gifv", "url": "https://tenor.com/view/cat", "thumbnail": proxied("cat.png", 498, 280),
             "video": {"url": "https://tenor.com/cat.mp4"}},
            {"type": "poll_result", "fields": []},
        ]));
        assert_eq!(parsed.embeds.len(), 3);
        assert_eq!(parsed.embeds[0].provider.as_deref(), Some("Example"));
        assert_eq!(parsed.embeds[0].color, Some(0x123456));
        assert!(parsed.embeds[2].video);
        let shown: Vec<_> = parsed
            .pictures
            .iter()
            .map(|picture| (picture.name.as_str(), picture.shown, picture.image))
            .collect();
        assert_eq!(
            shown,
            [
                ("news.jpg", Shown::Embed(0), true),
                ("logo.png", Shown::Thumbnail(1), true),
                ("poster.jpg", Shown::Poster(2), false),
                ("cat.png", Shown::Message, true),
            ]
        );
        assert!(
            parsed.pictures[0]
                .preview_url
                .as_deref()
                .unwrap()
                .starts_with("https://images-ext-1.discordapp.net/")
        );
        assert_eq!(parsed.hides, ["https://tenor.com/view/cat"]);
    }

    #[test]
    fn bot_cards_keep_their_fields_footer_and_shared_pictures() {
        let parsed = parse(&json!([
            {"type": "rich", "url": "https://example.com/post", "title": "Post",
             "fields": [{"name": "Score", "value": "**3**", "inline": true},
                        {"name": "Notes", "value": "none"}],
             "footer": {"text": "Bot"}, "timestamp": "2026-10-06T12:00:00+00:00",
             "image": proxied("one.png", 800, 600)},
            {"type": "rich", "url": "https://example.com/post", "image": proxied("two.png", 800, 600)},
        ]));
        assert_eq!(
            parsed.embeds.len(),
            1,
            "the second picture joins the first card"
        );
        let card = &parsed.embeds[0];
        assert_eq!(card.fields.len(), 2);
        assert!(card.fields[0].inline && !card.fields[1].inline);
        assert_eq!(card.footer.as_deref(), Some("Bot • 6 Oct 2026"));
        assert!(
            parsed
                .pictures
                .iter()
                .all(|picture| picture.shown == Shown::Embed(0))
        );
        assert_eq!(parsed.pictures.len(), 2);
    }
}
