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
        DiscordCommand::GifPicture { url, width, height } => {
            tokio::spawn(async move {
                let picture = match discord.download(&url, MAX_GIF_BYTES).await {
                    Ok(data) => tokio::task::spawn_blocking(move || {
                        crate::chat::decode_picture(&data, width, height)
                    })
                    .await
                    .ok()
                    .flatten()
                    .map(Arc::new),
                    Err(error) => {
                        tracing::debug!(%error, %url, "could not download a GIF");
                        None
                    }
                };
                let _ = events.send(Event::GifPicture { url, picture });
            });
        }
        _ => {}
    }
    None
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
