//! The GIF picker's commands: trending terms, searches, suggestions, the
//! account's favourites, and the pictures the picker shows.

use super::*;
use crate::chat::{FavoriteGif, Gif, GifCategory};

/// The largest GIF picture the picker downloads. A favourite from another
/// client may be a full-size `.gif`.
const MAX_GIF_BYTES: usize = 8 * 1024 * 1024;

/// Carries out a GIF picker command, or gives back any other.
pub(super) fn command(
    command: Command,
    discord: Result<&Discord, &str>,
    events: &UnboundedSender<Event>,
) -> Option<Command> {
    let Command::Discord(command) = command else {
        return Some(command);
    };
    let discord = match (&command, discord) {
        (
            DiscordCommand::GifCategories
            | DiscordCommand::Gifs(_)
            | DiscordCommand::GifSuggestions(_)
            | DiscordCommand::GifPicked { .. }
            | DiscordCommand::FavoriteGifs
            | DiscordCommand::SetFavoriteGifs(_)
            | DiscordCommand::GifPicture { .. },
            Ok(discord),
        ) => discord.clone(),
        (DiscordCommand::GifCategories, Err(why)) => {
            let _ = events.send(Event::GifCategories(Err(why.into())));
            return None;
        }
        (DiscordCommand::Gifs(query), Err(why)) => {
            let _ = events.send(Event::Gifs {
                query: query.clone(),
                result: Err(why.into()),
            });
            return None;
        }
        (DiscordCommand::FavoriteGifs, Err(why)) => {
            let _ = events.send(Event::FavoriteGifs(Err(why.into())));
            return None;
        }
        (
            DiscordCommand::GifSuggestions(_)
            | DiscordCommand::GifPicked { .. }
            | DiscordCommand::SetFavoriteGifs(_)
            | DiscordCommand::GifPicture { .. },
            Err(_),
        ) => return None,
        _ => return Some(Command::Discord(command)),
    };
    let events = events.clone();
    match command {
        DiscordCommand::GifCategories => {
            tokio::spawn(async move {
                let result = discord
                    .gifs_trending()
                    .await
                    .map(|trending| {
                        let categories = trending
                            .categories
                            .into_iter()
                            .map(|category| GifCategory {
                                name: category.name,
                                src: category.src,
                            })
                            .collect();
                        (categories, trending.preview.map(gif))
                    })
                    .map_err(|error| format!("Couldn't load GIFs: {error}"));
                let _ = events.send(Event::GifCategories(result));
            });
        }
        DiscordCommand::Gifs(query) => {
            tokio::spawn(async move {
                let found = match &query {
                    Some(query) => discord.gifs_search(query).await,
                    None => discord.gifs_trending_gifs().await,
                };
                let result = found
                    .map(|gifs| gifs.into_iter().map(gif).collect())
                    .map_err(|error| format!("Couldn't load GIFs: {error}"));
                let _ = events.send(Event::Gifs { query, result });
            });
        }
        DiscordCommand::GifSuggestions(query) => {
            tokio::spawn(async move {
                // As Discord's client asks: five, which show under the results.
                match discord.gifs_suggest(&query, 5).await {
                    Ok(terms) => {
                        let _ = events.send(Event::GifSuggestions { query, terms });
                    }
                    Err(error) => tracing::debug!(%error, "could not suggest GIF searches"),
                }
            });
        }
        DiscordCommand::GifPicked { id, query } => {
            tokio::spawn(async move {
                if let Err(error) = discord.gif_select(&id, &query).await {
                    tracing::debug!(%error, "could not report the GIF picked");
                }
            });
        }
        DiscordCommand::FavoriteGifs => {
            tokio::spawn(async move {
                if let Some(kept) = discord.kept_favorite_gifs().await {
                    let kept = kept.into_iter().map(favorite).collect();
                    let _ = events.send(Event::KeptFavoriteGifs(kept));
                }
                let result = discord
                    .favorite_gifs()
                    .await
                    .map(|favorites| favorites.into_iter().map(favorite).collect())
                    .map_err(|error| format!("Couldn't load your favourite GIFs: {error}"));
                let _ = events.send(Event::FavoriteGifs(result));
            });
        }
        DiscordCommand::SetFavoriteGifs(favorites) => {
            tokio::spawn(async move {
                let kept: Vec<talk_discord::gifs::Favorite> = favorites
                    .into_iter()
                    .map(|favorite| talk_discord::gifs::Favorite {
                        url: favorite.url,
                        src: favorite.src,
                        video: favorite.video,
                        width: favorite.width,
                        height: favorite.height,
                        order: favorite.order,
                    })
                    .collect();
                if let Err(error) = discord.set_favorite_gifs(&kept).await {
                    let _ = events.send(Event::Error(format!(
                        "Couldn't save your favourite GIFs: {error}"
                    )));
                }
            });
        }
        DiscordCommand::GifPicture {
            url,
            width,
            height,
            favorite,
        } => {
            tokio::spawn(async move {
                let picture = if favorite {
                    favorite_picture(&discord, &url, width, height).await
                } else {
                    picture(&discord, &url, width, height).await
                };
                let _ = events.send(Event::GifPicture {
                    url,
                    picture: picture.map(Arc::new),
                });
            });
        }
        _ => {}
    }
    None
}

/// The GIF picture at `url`, fit to `width` by `height`.
async fn picture(discord: &Discord, url: &str, width: u32, height: u32) -> Option<Picture> {
    match discord.download(url, MAX_GIF_BYTES).await {
        Ok(data) => {
            tokio::task::spawn_blocking(move || crate::chat::decode_picture(&data, width, height))
                .await
                .ok()
                .flatten()
        }
        Err(error) => {
            tracing::debug!(%error, %url, "could not download a GIF");
            None
        }
    }
}

/// A favourite GIF's picture, from the thumbnail kept of it, which this
/// makes the first time; its frames read from the file as they show.
async fn favorite_picture(
    discord: &Discord,
    url: &str,
    width: u32,
    height: u32,
) -> Option<Picture> {
    let lookup = url.to_owned();
    let kept = tokio::task::spawn_blocking(move || {
        talk_discord::cache::kept_gif(&lookup)
            .map(|kept| crate::chat::decode_picture_at(&kept, width, height))
    })
    .await
    .ok()?;
    if let Some(picture) = kept {
        return picture;
    }
    let made = make_thumbnail(discord, url, width).await?;
    tokio::task::spawn_blocking(move || crate::chat::decode_picture_at(&made, width, height))
        .await
        .ok()?
}

/// The most a favourite's picture downloads to make its thumbnail from:
/// Tenor's full-size GIFs took up to 24 MB.
const MAX_FAVORITE_BYTES: u64 = 40 * 1024 * 1024;
/// A favourite's picture this small, or a video, is its own thumbnail. A
/// small GIF keeps its transparency, which VP9 can't.
const KEPT_AS_IS: u64 = 64 * 1024;
/// A picture no thumbnail could be made of is kept if it's no larger.
const KEPT_UNMADE: u64 = 8 * 1024 * 1024;

/// Makes the thumbnail of the favourite whose picture is at `url`, for tiles
/// `width` wide, from Tenor's small MP4 when it's Tenor's, or from the
/// picture itself, which downloads to disk as it arrives.
async fn make_thumbnail(discord: &Discord, url: &str, width: u32) -> Option<PathBuf> {
    use std::io::Seek;
    let target = url.to_owned();
    let (download, mut file) =
        tokio::task::spawn_blocking(move || talk_discord::cache::gif_download(&target))
            .await
            .ok()??;
    let mut downloaded = false;
    for source in talk_discord::gifs::small_video(url)
        .into_iter()
        .chain([url.to_owned()])
    {
        if file.set_len(0).and_then(|()| file.rewind()).is_err() {
            break;
        }
        match discord
            .download_into(&source, &mut file, MAX_FAVORITE_BYTES)
            .await
        {
            Ok(_) => {
                downloaded = true;
                break;
            }
            Err(error) => tracing::debug!(%error, %source, "could not download a favourite GIF"),
        }
    }
    drop(file);
    let url = url.to_owned();
    tokio::task::spawn_blocking(move || {
        let made = downloaded
            .then(|| thumbnail(&url, &download, width))
            .flatten();
        let _ = std::fs::remove_file(&download);
        made
    })
    .await
    .ok()?
}

/// Keeps the picture downloaded to `download` as the favourite's thumbnail:
/// itself when it's a video or small, and otherwise a VP9 WebM of it at
/// `width`, a tenth of a WebP's size.
fn thumbnail(url: &str, download: &std::path::Path, width: u32) -> Option<PathBuf> {
    use talk_discord::cache::{KeptGif, keep_gif};
    let size = std::fs::metadata(download).ok()?.len();
    let mut start = [0; 8];
    let video = std::io::Read::read_exact(&mut std::fs::File::open(download).ok()?, &mut start)
        .is_ok()
        && crate::video::is_video(&start);
    if video || size <= KEPT_AS_IS {
        return keep_gif(url, KeptGif::File(download));
    }
    let made = crate::chat::decode_picture_at(download, width, width.saturating_mul(4))
        .and_then(|picture| crate::clip::thumbnail(&picture));
    match made {
        Some(webm) => keep_gif(url, KeptGif::Bytes(&webm)),
        None if size <= KEPT_UNMADE => keep_gif(url, KeptGif::File(download)),
        None => None,
    }
}

/// A message's picture as a GIF to keep among the favourites, as Discord's
/// client offers it: a linked GIF by its page, or an attached `.gif` by its
/// address without the signature that expires.
pub(crate) fn favorite_gif(attachment: &talk_discord::message::Attachment) -> Option<Gif> {
    let (width, height) = attachment.dimensions.unwrap_or_default();
    let (url, src) = if let Some(page) = &attachment.page_url {
        (page.clone(), attachment.url.clone())
    } else if attachment.image && attachment.name.to_lowercase().ends_with(".gif") {
        let unsigned = unsigned(&attachment.url);
        (unsigned.clone(), unsigned)
    } else {
        return None;
    };
    Some(Gif {
        id: None,
        url,
        src,
        gif_src: String::new(),
        width,
        height,
    })
}

/// A Discord file's address without `ex`, `is` and `hm`, which sign it for
/// a while.
fn unsigned(url: &str) -> String {
    let Some((path, query)) = url.split_once('?') else {
        return url.to_owned();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| !matches!(pair.split('=').next(), Some("ex" | "is" | "hm")))
        .collect();
    if kept.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
}

/// The favourites another client changed, as Talk shows them.
pub(super) fn favorites(favorites: &[talk_discord::gifs::Favorite]) -> Vec<FavoriteGif> {
    favorites.iter().cloned().map(favorite).collect()
}

fn gif(gif: talk_discord::gifs::Gif) -> Gif {
    Gif {
        id: gif.id,
        url: gif.url,
        src: gif.src,
        gif_src: gif.gif_src,
        width: gif.width,
        height: gif.height,
    }
}

fn favorite(favorite: talk_discord::gifs::Favorite) -> FavoriteGif {
    FavoriteGif {
        url: favorite.url,
        src: favorite.src,
        video: favorite.video,
        width: favorite.width,
        height: favorite.height,
        order: favorite.order,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GIF of noise, which compresses badly, so `frames` frames make a
    /// large file.
    fn noisy_gif(frames: u32) -> Vec<u8> {
        let mut seed = 7u32;
        let mut data = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut data);
            for _ in 0..frames {
                let picture = image::RgbaImage::from_fn(64, 48, |_, _| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let [r, g, b, _] = seed.to_le_bytes();
                    image::Rgba([r, g, b, 255])
                });
                encoder
                    .encode_frame(image::Frame::from_parts(
                        picture,
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(80, 1),
                    ))
                    .unwrap();
            }
        }
        data
    }

    #[test]
    fn keeps_a_favourites_thumbnail_as_vp9_unless_small() {
        let directory = tempfile::tempdir().unwrap();
        talk_discord::cache::keep_in(directory.path().join("discord-account.json"));
        let download = |url: &str, data: &[u8]| {
            let (path, mut file) = talk_discord::cache::gif_download(url).unwrap();
            std::io::Write::write_all(&mut file, data).unwrap();
            path
        };

        // A large GIF becomes a WebM at the tiles' width, its frames timed
        // as they were.
        let large = noisy_gif(30);
        assert!(large.len() as u64 > KEPT_AS_IS);
        let url = "https://media.tenor.com/abc/large.gif";
        let kept = thumbnail(url, &download(url, &large), 32).unwrap();
        assert_eq!(talk_discord::cache::kept_gif(url), Some(kept.clone()));
        let webm = std::fs::read(&kept).unwrap();
        assert!(crate::video::is_video(&webm) && webm.len() < large.len() / 4);
        let picture = crate::chat::decode_picture_at(&kept, 32, 128).unwrap();
        assert_eq!((picture.width, picture.height), (32, 24));
        assert_eq!(picture.animation.as_ref().unwrap().delays, [80; 30]);
        assert_eq!(picture.frames().count(), 30);

        // A small one stays as it is.
        let small = noisy_gif(2);
        assert!(small.len() as u64 <= KEPT_AS_IS);
        let url = "https://media.tenor.com/abc/small.gif";
        let kept = thumbnail(url, &download(url, &small), 32).unwrap();
        assert_eq!(std::fs::read(kept).unwrap(), small);
    }
}
