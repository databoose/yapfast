//! GIFs as Discord's picker finds them, and the account's favourites.
//!
//! Discord asks its GIF provider, Klipy now, through `/gifs/*`: trending
//! categories for the picker's home, a search, and terms to suggest when a
//! search finds little. Each GIF comes in the format asked for; Talk asks for
//! `tinywebp`, as Discord's Linux client does, which its picture decoder
//! animates. Favourites live in the second settings protobuf,
//! `FrecencyUserSettings.favorite_gifs`, keyed by the GIF's page address.

use base64::Engine;
use serde_json::Value;

use crate::settings::{Field, VARINT, fields, put_bytes, put_varint};

/// The format the picker asks for: small animated WebP.
pub const MEDIA_FORMAT: &str = "tinywebp";
/// The language of category names and suggestions.
pub const LOCALE: &str = "en-US";

/// Where GIFs' pictures come from, which Discord's client loads directly.
pub const HOSTS: [&str; 6] = [
    "media.tenor.com",
    "media.tenor.co",
    "c.tenor.com",
    "static.klipy.com",
    "media.giphy.com",
    "i.giphy.com",
];

/// Whether `url` is a GIF provider's picture, which Talk may download.
pub fn provider_media(url: &str) -> bool {
    let host = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default();
    HOSTS.contains(&host)
        || host
            .strip_prefix("media")
            .and_then(|rest| rest.strip_suffix(".giphy.com"))
            .is_some_and(|number| number.bytes().all(|b| b.is_ascii_digit()))
}

/// A GIF the provider found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Gif {
    /// The provider's id, which `/gifs/select` reports a pick by.
    pub id: Option<String>,
    /// Its page, which a message sends and favourites are keyed by.
    pub url: String,
    /// Its picture in the format asked for.
    pub src: String,
    /// The same as a `.gif`.
    pub gif_src: String,
    pub width: u32,
    pub height: u32,
}

impl Gif {
    pub fn parse(value: &Value) -> Option<Self> {
        let text = |key: &str| value[key].as_str().map(https).unwrap_or_default();
        let url = value["url"].as_str()?.to_owned();
        let src = text("src");
        if src.is_empty() {
            return None;
        }
        Some(Self {
            id: match &value["id"] {
                Value::String(id) => Some(id.clone()),
                Value::Number(id) => Some(id.to_string()),
                _ => None,
            },
            url,
            src,
            gif_src: text("gif_src"),
            width: dimension(&value["width"]),
            height: dimension(&value["height"]),
        })
    }
}

/// A trending search term, with a GIF of it for its tile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Category {
    pub name: String,
    pub src: String,
}

/// The picker's home: trending terms, and a GIF for its "Trending GIFs" tile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trending {
    pub categories: Vec<Category>,
    pub preview: Option<Gif>,
}

impl Trending {
    pub fn parse(value: &Value) -> Self {
        Self {
            categories: value["categories"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|category| {
                    Some(Category {
                        name: category["name"].as_str()?.to_owned(),
                        src: https(category["src"].as_str()?),
                    })
                })
                .collect(),
            preview: value["gifs"]
                .as_array()
                .and_then(|gifs| gifs.first())
                .and_then(Gif::parse),
        }
    }
}

pub fn parse_list(value: &Value) -> Vec<Gif> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Gif::parse)
        .collect()
}

fn dimension(value: &Value) -> u32 {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
}

/// An address the provider gave without its scheme, `//media...`, with it.
fn https(url: &str) -> String {
    match url.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_owned(),
    }
}

/// A favourite GIF, as the settings keep it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Favorite {
    /// Its page, the key.
    pub url: String,
    /// The picture shown for it.
    pub src: String,
    /// The picture is a video rather than an image.
    pub video: bool,
    pub width: u32,
    pub height: u32,
    /// When it was added: one more than the newest before it.
    pub order: u32,
}

/// `FrecencyUserSettings.favorite_gifs`, a `FavoriteGIFs`: its `gifs`, a
/// map from page address to `FavoriteGIF`, and `hide_tooltip`.
const FAVORITE_GIFS: u64 = 2;
const GIFS: u64 = 1;
const HIDE_TOOLTIP: u64 = 2;
/// A map entry's key and value.
const KEY: u64 = 1;
const VALUE: u64 = 2;
/// `FavoriteGIF`'s fields; `format` is 1 for an image and 2 for a video.
const FORMAT: u64 = 1;
const SRC: u64 = 2;
const WIDTH: u64 = 3;
const HEIGHT: u64 = 4;
const ORDER: u64 = 5;
const VIDEO: u64 = 2;

/// The favourites in base64 `FrecencyUserSettings`, as GET
/// `/users/@me/settings-proto/2` or USER_SETTINGS_PROTO_UPDATE carries them;
/// `None` when they don't say, as a partial update may not.
pub fn read_favorites(encoded: &str) -> Option<Vec<Favorite>> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let favorites = fields(&data).find_map(|(number, value)| match value {
        Field::Bytes(bytes) if number == FAVORITE_GIFS => Some(bytes),
        _ => None,
    })?;
    let mut read = Vec::new();
    for (number, value) in fields(favorites) {
        let (GIFS, Field::Bytes(entry)) = (number, value) else {
            continue;
        };
        let mut favorite = Favorite::default();
        for (number, value) in fields(entry) {
            match (number, value) {
                (KEY, Field::Bytes(key)) => favorite.url = String::from_utf8_lossy(key).into(),
                (VALUE, Field::Bytes(gif)) => {
                    for (number, value) in fields(gif) {
                        match (number, value) {
                            (FORMAT, Field::Varint(format)) => favorite.video = format == VIDEO,
                            (SRC, Field::Bytes(src)) => {
                                favorite.src = String::from_utf8_lossy(src).into();
                            }
                            (WIDTH, Field::Varint(width)) => favorite.width = width as u32,
                            (HEIGHT, Field::Varint(height)) => favorite.height = height as u32,
                            (ORDER, Field::Varint(order)) => favorite.order = order as u32,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        if !favorite.url.is_empty() {
            read.push(favorite);
        }
    }
    Some(read)
}

/// The favourites as base64 `FrecencyUserSettings` holding only them, which
/// a PATCH of `/users/@me/settings-proto/2` puts in place of the old. The
/// tooltip that explains them hides once there are more than two, as
/// Discord's client hides it.
pub fn write_favorites(favorites: &[Favorite]) -> String {
    let mut message = Vec::new();
    for favorite in favorites {
        let mut gif = Vec::new();
        put_varint(&mut gif, FORMAT << 3 | VARINT);
        put_varint(&mut gif, if favorite.video { VIDEO } else { 1 });
        put_bytes(&mut gif, SRC, favorite.src.as_bytes());
        for (number, value) in [
            (WIDTH, favorite.width),
            (HEIGHT, favorite.height),
            (ORDER, favorite.order),
        ] {
            put_varint(&mut gif, number << 3 | VARINT);
            put_varint(&mut gif, u64::from(value));
        }
        let mut entry = Vec::new();
        put_bytes(&mut entry, KEY, favorite.url.as_bytes());
        put_bytes(&mut entry, VALUE, &gif);
        put_bytes(&mut message, GIFS, &entry);
    }
    if favorites.len() > 2 {
        put_varint(&mut message, HIDE_TOOLTIP << 3 | VARINT);
        put_varint(&mut message, 1);
    }
    let mut settings = Vec::new();
    put_bytes(&mut settings, FAVORITE_GIFS, &message);
    base64::engine::general_purpose::STANDARD.encode(settings)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn favorites_go_and_come_back_as_discords_settings_keep_them() {
        let favorites = vec![
            Favorite {
                url: "https://klipy.com/gifs/cat".into(),
                src: "https://static.klipy.com/cat.gif".into(),
                video: false,
                width: 220,
                height: 124,
                order: 1,
            },
            Favorite {
                url: "https://klipy.com/gifs/dog".into(),
                src: "https://static.klipy.com/dog.mp4".into(),
                video: true,
                width: 220,
                height: 220,
                order: 2,
            },
        ];
        assert_eq!(
            read_favorites(&write_favorites(&favorites)).as_deref(),
            Some(&favorites[..])
        );
        // Settings without favourites don't say.
        assert_eq!(read_favorites(""), None);
    }

    #[test]
    fn reads_trending_and_finds_only_providers_pictures() {
        let trending = Trending::parse(&json!({
            "categories": [{"name": "happy", "src": "//static.klipy.com/happy.webp"}],
            "gifs": [{"id": 1, "url": "https://klipy.com/gifs/a", "src": "https://static.klipy.com/a.webp", "width": 10, "height": 20}],
        }));
        assert_eq!(
            trending.categories[0].src,
            "https://static.klipy.com/happy.webp"
        );
        assert_eq!(trending.preview.unwrap().id.as_deref(), Some("1"));
        assert!(provider_media("https://static.klipy.com/a.webp"));
        assert!(provider_media("https://media2.giphy.com/media/x/giphy.gif"));
        assert!(!provider_media("https://example.com/a.gif"));
        assert!(!provider_media("http://static.klipy.com/a.webp"));
    }
}
