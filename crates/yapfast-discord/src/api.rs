//! Discord's HTTP API, called the way the web client calls it.

use std::sync::RwLock;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, header};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    captcha::Captcha,
    client,
    limits::{self, Limits, Refusals, Stopped},
    settings::{self, CustomStatus},
};

const API: &str = "https://discord.com/api/v9";
/// The web client's page, which carries its build number.
const WEB_APP: &str = "https://discord.com/app";
const UNAUTHORIZED: &str = "Discord rejected the login. Link Discord again.";
/// How a message goes, besides what it says.
#[derive(Clone, Copy, Debug)]
pub struct Sending<'a> {
    /// Matches the message to its echo from the gateway.
    pub nonce: &'a str,
    /// The message it replies to, in the same channel.
    pub reply: Option<&'a str>,
    /// Whether a reply notifies the author of what it replies to.
    pub ping_reply: bool,
    /// Notifies nobody, as `@silent` does.
    pub silent: bool,
}

/// The message flag of `@silent`: no push or desktop notifications.
const SUPPRESS_NOTIFICATIONS: u64 = 1 << 12;
/// The most messages one history request returns.
pub const PAGE: usize = 50;

/// How many archived threads a page of a channel's list of threads holds,
/// as Discord's client asks for them.
pub const THREAD_PAGE: u32 = 25;

/// Which of a channel's messages a page holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span<'a> {
    /// The newest.
    Latest,
    /// Those before a snowflake.
    Before(&'a str),
    /// Those after a snowflake, which Discord's client asks for as the
    /// timeline scrolls down through older messages.
    After(&'a str),
    /// Those on both sides of a snowflake, as for a jump to a message;
    /// Discord answers even when no message has it.
    Around(&'a str),
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
}

/// A file uploaded to Discord's storage, ready to attach to a message.
pub struct Upload {
    pub filename: String,
    pub uploaded_filename: String,
}

/// The most files one message can carry.
pub const MAX_FILES: usize = 10;

/// The most channels one bulk read acknowledgement carries.
pub const ACK_BULK_LIMIT: usize = 100;

const MIB: u64 = 1024 * 1024;

/// The largest file this account may send, in bytes: by its Nitro, as
/// [`crate::service::Account::premium_type`], or in a server boosted to
/// `premium_tier` 2 or 3, by the server's, whichever allows more. These are
/// Discord's limits since August 2026. Its client's own constant without
/// Nitro is still 10 MiB, but the client raises every limit to at least
/// 20 MiB before checking a file or naming the limit.
pub fn upload_limit(premium_type: u8, premium_tier: Option<u8>) -> u64 {
    let own = match premium_type {
        2 => 500 * MIB,
        1 | 3 => 50 * MIB,
        _ => 20 * MIB,
    };
    let boosted = match premium_tier {
        Some(3) => 100 * MIB,
        Some(2) => 50 * MIB,
        _ => 0,
    };
    own.max(boosted)
}

/// Discord answered a download with this error status.
#[derive(Debug)]
pub struct Refused(pub StatusCode);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Discord answered {}", self.0)
    }
}

impl std::error::Error for Refused {}

/// Whether a failed [`Api::download`] may work if tried later: Discord was
/// out of reach, failed or asked Yapfast to slow down, rather than refusing the
/// file or Yapfast refusing what came.
pub fn may_work_later(error: &anyhow::Error) -> bool {
    match error.downcast_ref::<Refused>() {
        Some(Refused(status)) => {
            status.is_server_error()
                || matches!(
                    *status,
                    StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
                )
        }
        None => error.downcast_ref::<reqwest::Error>().is_some(),
    }
}

pub struct Api {
    http: Client,
    /// Empty without a login; Discord may refresh it in READY.
    token: RwLock<String>,
    /// The API's base URL: Discord's, or the test server's.
    base: String,
    limits: Limits,
    refusals: Refusals,
    /// Downloads going at once, so opening the emoji picker or a busy channel
    /// spreads its pictures out rather than asking for hundreds together.
    downloads: tokio::sync::Semaphore,
}

/// The most files Yapfast downloads from Discord at once. They share one HTTP/2
/// connection, as a browser's requests to one host do.
const DOWNLOADS: usize = 8;

impl Api {
    pub fn new(token: String) -> Result<Self> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let http = Client::builder()
            .user_agent(client::user_agent())
            .cookie_provider(crate::cookies::jar())
            .build()
            .context("could not set up HTTPS")?;
        let base = match client::test_server() {
            Some(server) => format!("{server}/api/v9"),
            None => API.to_owned(),
        };
        Ok(Self {
            http,
            token: RwLock::new(token),
            base,
            limits: Limits::default(),
            refusals: Refusals::default(),
            downloads: tokio::sync::Semaphore::new(DOWNLOADS),
        })
    }

    /// Without a login, for linking one.
    pub fn anonymous() -> Result<Self> {
        Self::new(String::new())
    }

    /// Reads the web client's current build number from its page, as a
    /// browser loads that page before the client connects. Once a run is
    /// enough; until then, and if the page can't be read, Yapfast sends the
    /// last one it knew.
    pub async fn learn_build_number(&self) {
        /// The page sets it near the top; the whole page is about 75 KiB.
        const READ: usize = 256 << 10;
        if client::test_server().is_some() || client::knows_build_number() {
            return;
        }
        let read = async {
            let mut response = self
                .http
                .get(WEB_APP)
                .header(header::ACCEPT, "text/html")
                .send()
                .await?
                .error_for_status()?;
            let mut page = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                page.extend_from_slice(&chunk);
                if client::learn_build_number(&page) || page.len() >= READ {
                    break;
                }
            }
            anyhow::Ok(())
        };
        match tokio::time::timeout(std::time::Duration::from_secs(10), read).await {
            Ok(Ok(())) if client::knows_build_number() => {}
            Ok(Ok(())) => tracing::warn!("Discord's web page gave no build number"),
            Ok(Err(error)) => tracing::warn!(%error, "could not read Discord's web page"),
            Err(_) => tracing::warn!("Discord's web page didn't load"),
        }
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        // Only the API gets the client's description, as in the web client;
        // it changes through a run, so each request makes it again.
        let request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("X-Super-Properties", client::super_properties());
        let token = self.token();
        if token.is_empty() {
            return request;
        }
        request.header(header::AUTHORIZATION, token.as_str())
    }

    fn token(&self) -> std::sync::RwLockReadGuard<'_, String> {
        self.token
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Asks with `token` from now on: Discord refreshed the login to it.
    pub fn relogin(&self, token: &str) {
        *self
            .token
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = token.to_owned();
    }

    /// Trades a remote login's ticket for the login, encrypted with the key
    /// that started it, in base64; see [`crate::remote_auth`]. Discord may
    /// ask for a [`Captcha`] first, whose solution goes with the next try.
    pub async fn remote_auth_login(
        &self,
        ticket: &str,
        solved: Option<(&Captcha, &str)>,
    ) -> Result<String> {
        let body = json!({"ticket": ticket});
        let mut request = self.request(Method::POST, "/users/@me/remote-auth/login");
        if let Some((captcha, solution)) = solved {
            for (name, value) in captcha.headers(solution) {
                request = request.header(name, value);
            }
        }
        let answer = self.json(with_json(request, &body)).await?;
        answer["encrypted_token"]
            .as_str()
            .map(str::to_owned)
            .context("Discord didn't hand over the login.")
    }

    /// Ends this login's session on Discord, as signing out of the web client
    /// does. The token stops working.
    pub async fn logout(&self) -> Result<()> {
        let body = json!({"provider": null, "voip_provider": null});
        self.empty(with_json(self.request(Method::POST, "/auth/logout"), &body))
            .await
    }

    /// The account the token belongs to.
    pub async fn me(&self) -> Result<User> {
        let body = self.json(self.request(Method::GET, "/users/@me")).await?;
        serde_json::from_value(body).context("could not understand Discord's answer")
    }

    /// Up to [`PAGE`] messages of a channel, newest first, as [`Span`] says.
    pub async fn messages(&self, channel: &str, span: Span<'_>) -> Result<Vec<Value>> {
        let mut path = format!("/channels/{channel}/messages?limit={PAGE}");
        match span {
            Span::Latest => {}
            Span::Before(id) => path.push_str(&format!("&before={id}")),
            Span::After(id) => path.push_str(&format!("&after={id}")),
            Span::Around(id) => path.push_str(&format!("&around={id}")),
        }
        match self.json(self.request(Method::GET, &path)).await? {
            Value::Array(messages) => Ok(messages),
            _ => bail!("could not understand Discord's answer"),
        }
    }

    /// Sends a message, as a reply when `options` names one in the same
    /// channel, with uploaded files.
    pub async fn send(
        &self,
        guild: Option<&str>,
        channel: &str,
        content: &str,
        options: Sending<'_>,
        uploads: &[Upload],
    ) -> Result<Value> {
        let mut body = message_body(content, options.nonce);
        if options.silent {
            body["flags"] = SUPPRESS_NOTIFICATIONS.into();
        }
        if !uploads.is_empty() {
            body["attachments"] = uploads
                .iter()
                .enumerate()
                .map(|(index, upload)| {
                    json!({
                        "id": index.to_string(),
                        "filename": upload.filename,
                        "uploaded_filename": upload.uploaded_filename,
                    })
                })
                .collect();
        }
        if let Some(reply) = options.reply {
            body["message_reference"] = json!({
                "guild_id": guild,
                "channel_id": channel,
                "message_id": reply,
                "type": 0,
            });
            // As Discord's client asks for a reply with its mention off:
            // everyone the text mentions, but not the author replied to.
            if !options.ping_reply {
                body["allowed_mentions"] = json!({
                    "parse": ["users", "roles", "everyone"],
                    "replied_user": false,
                });
            }
        }
        let path = format!("/channels/{channel}/messages");
        self.json(with_json(self.request(Method::POST, &path), &body))
            .await
    }

    /// Sends a message of text alone from a menu, such as the link Invite
    /// to Server sends, saying so as Discord's client does.
    pub async fn send_from_menu(&self, channel: &str, content: &str, nonce: &str) -> Result<Value> {
        let path = format!("/channels/{channel}/messages");
        let request = with_json(
            self.request(Method::POST, &path),
            &message_body(content, nonce),
        )
        .header("X-Context-Properties", context("context_menu"));
        self.json(request).await
    }

    /// A user's profile, with what they share with this account, and their
    /// profile in `guild`.
    pub async fn profile(&self, user: &str, guild: Option<&str>) -> Result<Value> {
        let mut path = format!(
            "/users/{user}/profile?with_mutual_guilds=true&with_mutual_friends=true&with_mutual_friends_count=false"
        );
        if let Some(guild) = guild {
            path.push_str(&format!("&guild_id={guild}"));
        }
        self.json(self.request(Method::GET, &path)).await
    }

    /// Accepts a message request, which makes it an ordinary DM.
    pub async fn accept_request(&self, channel: &str) -> Result<()> {
        let path = format!("/channels/{channel}/recipients/@me");
        // 2 is ACCEPTED.
        let body = json!({"consent_status": 2});
        self.empty(with_json(self.request(Method::PUT, &path), &body))
            .await
    }

    /// Ignores a message request: it leaves this account's DMs, and the
    /// sender isn't told.
    pub async fn ignore_request(&self, channel: &str) -> Result<()> {
        let path = format!("/channels/{channel}/recipients/@me");
        self.empty(self.request(Method::DELETE, &path)).await
    }

    /// Uploads a file to Discord's storage, as the official client does
    /// before sending a message with it.
    pub async fn upload(&self, channel: &str, filename: &str, data: Vec<u8>) -> Result<Upload> {
        let path = format!("/channels/{channel}/attachments");
        let body = json!({"files": [{
            "id": "0",
            "filename": filename,
            "file_size": data.len(),
            "is_clip": false,
        }]});
        let answer = self
            .json(with_json(self.request(Method::POST, &path), &body))
            .await?;
        let target = &answer["attachments"][0];
        let (Some(url), Some(uploaded_filename)) = (
            target["upload_url"].as_str(),
            target["upload_filename"].as_str(),
        ) else {
            bail!("could not understand Discord's answer");
        };
        // The upload URL is signed, so it takes no login.
        let response = self
            .http
            .put(url)
            .body(data)
            .send()
            .await
            .context("Can't reach Discord's file storage.")?;
        if !response.status().is_success() {
            bail!("Discord's file storage answered {}.", response.status());
        }
        Ok(Upload {
            filename: filename.to_owned(),
            uploaded_filename: uploaded_filename.to_owned(),
        })
    }

    /// Removes an uploaded file that no message used.
    pub async fn discard(&self, upload: &Upload) -> Result<()> {
        let name: Vec<_> = upload.uploaded_filename.split('/').map(encode).collect();
        let path = format!("/attachments/{}", name.join("/"));
        self.empty(self.request(Method::DELETE, &path)).await
    }

    pub async fn edit(&self, channel: &str, message: &str, content: &str) -> Result<Value> {
        let path = format!("/channels/{channel}/messages/{message}");
        let body = json!({"content": content});
        self.json(with_json(self.request(Method::PATCH, &path), &body))
            .await
    }

    /// Deletes a message. One Discord no longer has is as good as deleted:
    /// another client, or a moderator, got there first.
    pub async fn delete(&self, channel: &str, message: &str) -> Result<()> {
        let path = format!("/channels/{channel}/messages/{message}");
        match self.empty(self.request(Method::DELETE, &path)).await {
            Err(error)
                if error
                    .downcast_ref::<Refused>()
                    .is_some_and(|refused| refused.0 == StatusCode::NOT_FOUND) =>
            {
                Ok(())
            }
            deleted => deleted,
        }
    }

    /// Marks a channel read up to `message`, as the official client does
    /// when a channel is on screen.
    pub async fn ack(&self, channel: &str, message: &str) -> Result<()> {
        let path = format!("/channels/{channel}/messages/{message}/ack");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        // Days since Discord's epoch.
        let days = (crate::message::snowflake_at(now) >> 22) / 86_400_000;
        let body = json!({"token": null, "last_viewed": days});
        self.json(with_json(self.request(Method::POST, &path), &body))
            .await
            .map(drop)
    }

    /// Marks channels read, each up to its message, in one request, as the
    /// official client does for a whole server; at most [`ACK_BULK_LIMIT`].
    pub async fn ack_bulk(&self, reads: &[(String, String)]) -> Result<()> {
        let read_states: Vec<Value> = reads
            .iter()
            .map(|(channel, message)| json!({"channel_id": channel, "message_id": message}))
            .collect();
        let body = json!({"read_states": read_states});
        self.empty(with_json(
            self.request(Method::POST, "/read-states/ack-bulk"),
            &body,
        ))
        .await
    }

    /// Adds or removes this account's reaction; `emoji` is the emoji itself
    /// or `name:id` for a custom one.
    pub async fn react(&self, channel: &str, message: &str, emoji: &str, add: bool) -> Result<()> {
        let path = format!(
            "/channels/{channel}/messages/{message}/reactions/{}/@me?location=Message&type=0",
            encode(emoji)
        );
        let method = if add { Method::PUT } else { Method::DELETE };
        self.empty(self.request(method, &path)).await
    }

    /// Removes `user`'s reaction to a message, which takes Manage Messages
    /// for anyone else's; `emoji` is as [`Self::react`] takes it.
    pub async fn remove_reaction(
        &self,
        channel: &str,
        message: &str,
        emoji: &str,
        user: &str,
    ) -> Result<()> {
        let path = format!(
            "/channels/{channel}/messages/{message}/reactions/{}/0/{user}?location=Message&burst=false",
            encode(emoji)
        );
        self.empty(self.request(Method::DELETE, &path)).await
    }

    /// Who reacted to a message with `emoji` (itself, or `name:id` for a
    /// custom one), as user objects: up to `limit`, by user id, after the
    /// user `after`. Super reactions aren't among them.
    pub async fn reactors(
        &self,
        channel: &str,
        message: &str,
        emoji: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Value>> {
        let mut path = format!(
            "/channels/{channel}/messages/{message}/reactions/{}?limit={limit}&type=0",
            encode(emoji)
        );
        if let Some(after) = after {
            path.push_str(&format!("&after={after}"));
        }
        match self.json(self.request(Method::GET, &path)).await? {
            Value::Array(users) => Ok(users),
            _ => bail!("could not understand Discord's answer"),
        }
    }

    /// Opens the DM with `user`, or finds the one already open; returns its channel id.
    pub async fn open_dm(&self, user: &str) -> Result<String> {
        let body = json!({"recipients": [user]});
        let channel = self
            .json(with_json(
                self.request(Method::POST, "/users/@me/channels"),
                &body,
            ))
            .await?;
        channel["id"]
            .as_str()
            .map(str::to_owned)
            .context("Discord didn't open the DM.")
    }

    /// Makes a group DM with `recipients`, as adding friends to a one-to-one
    /// DM does, which stays as it was; returns its channel id.
    pub async fn create_group(&self, recipients: &[String]) -> Result<String> {
        let body = json!({"recipients": recipients});
        let channel = self
            .json(with_json(
                self.request(Method::POST, "/users/@me/channels"),
                &body,
            ))
            .await?;
        channel["id"]
            .as_str()
            .map(str::to_owned)
            .context("Discord didn't make the group.")
    }

    /// Adds `user` to a group DM.
    pub async fn add_recipient(&self, channel: &str, user: &str) -> Result<()> {
        let path = format!("/channels/{channel}/recipients/{user}");
        self.empty(with_json(self.request(Method::PUT, &path), &json!({})))
            .await
    }

    /// Pins a message to its channel's pinned messages, or unpins it.
    pub async fn pin(&self, channel: &str, message: &str, pinned: bool) -> Result<()> {
        let path = format!("/channels/{channel}/messages/pins/{message}");
        let method = if pinned { Method::PUT } else { Method::DELETE };
        self.empty(self.request(method, &path)).await
    }

    /// A channel's pinned messages, the latest pinned first: up to [`PAGE`].
    pub async fn pins(&self, channel: &str) -> Result<Vec<Value>> {
        let path = format!("/channels/{channel}/messages/pins?limit={PAGE}");
        let mut answer = self.json(self.request(Method::GET, &path)).await?;
        match answer["items"].take() {
            Value::Array(items) => Ok(items
                .into_iter()
                .map(|mut item| item["message"].take())
                .collect()),
            _ => bail!("could not understand Discord's answer"),
        }
    }

    /// Discord's answer to a search of messages, the newest first, from the
    /// `offset`th; see [`crate::service::Found`]. With a server, it searches
    /// the server, in the channels `params` names with `channel_id`, or all
    /// of them; without one, the DM `channel`. `params` are the rest of the
    /// query string, such as `content`, `author_id` and `has`, each name
    /// repeated for several values, as Discord's client sends them.
    pub async fn search(
        &self,
        guild: Option<&str>,
        channel: &str,
        params: &[(&str, String)],
        offset: u32,
    ) -> Result<Value> {
        let mut query = String::new();
        for (name, value) in params {
            query.push_str(&format!("{name}={}&", encode(value)));
        }
        query.push_str(&format!("offset={offset}"));
        let path = match guild {
            Some(guild) => format!("/guilds/{guild}/messages/search?{query}"),
            None => format!("/channels/{channel}/messages/search?{query}"),
        };
        self.json(self.request(Method::GET, &path)).await
    }

    /// Discord's answer to a search of a channel's threads, as its client's
    /// list of threads asks: the archived ones, the newest message first,
    /// [`THREAD_PAGE`] from the `offset`th; or with a `name`, those of
    /// either kind it matches.
    pub async fn search_threads(
        &self,
        channel: &str,
        name: Option<&str>,
        offset: u32,
    ) -> Result<Value> {
        let path = match name {
            Some(name) => format!(
                "/channels/{channel}/threads/search?name={}&tag_setting=match_some",
                encode(name)
            ),
            None => format!(
                "/channels/{channel}/threads/search?archived=true&sort_by=last_message_time\
                 &sort_order=desc&limit={THREAD_PAGE}&tag_setting=match_some&offset={offset}"
            ),
        };
        self.json(self.request(Method::GET, &path)).await
    }

    /// Starts a thread in `channel`, from its `message` or from none, with
    /// `body` as Discord's client sends it; the thread's channel object.
    /// Discord sends THREAD_CREATE, and adds this account to it.
    pub async fn create_thread(
        &self,
        channel: &str,
        message: Option<&str>,
        body: &Value,
    ) -> Result<Value> {
        let path = match message {
            Some(message) => format!("/channels/{channel}/messages/{message}/threads"),
            None => format!("/channels/{channel}/threads"),
        };
        self.json(with_json(self.request(Method::POST, &path), body))
            .await
    }

    /// Shows others that this account is typing in `channel`.
    pub async fn typing(&self, channel: &str) -> Result<()> {
        let path = format!("/channels/{channel}/typing");
        self.empty(self.request(Method::POST, &path)).await
    }

    /// Sends `user` a friend request, or accepts theirs. `place` is where in
    /// Discord's client this happens, which it tells Discord with each change
    /// to a relationship, such as "User Profile" or "Friends".
    pub async fn add_friend(&self, user: &str, place: &str) -> Result<()> {
        let path = format!("/users/@me/relationships/{user}");
        let request = with_json(self.request(Method::PUT, &path), &json!({}));
        self.empty(request.header("X-Context-Properties", context(place)))
            .await
    }

    /// Sends a friend request to whoever has `username`.
    pub async fn add_friend_by_username(&self, username: &str) -> Result<()> {
        let body = json!({"username": username, "discriminator": null});
        let request = with_json(
            self.request(Method::POST, "/users/@me/relationships"),
            &body,
        );
        self.empty(request.header("X-Context-Properties", context("Add Friend")))
            .await
    }

    /// Sets the custom status, or clears it with `None`, in the account's settings.
    pub async fn set_custom_status(&self, custom: Option<&CustomStatus>) -> Result<()> {
        let body = json!({"settings": settings::write_custom(custom)});
        self.json(with_json(
            self.request(Method::PATCH, "/users/@me/settings-proto/1"),
            &body,
        ))
        .await
        .map(drop)
    }

    /// The GIF picker's home: trending terms, each with a GIF, and trending GIFs.
    pub async fn gifs_trending(&self) -> Result<Value> {
        let path = format!(
            "/gifs/trending?media_format={}&locale={}",
            crate::gifs::MEDIA_FORMAT,
            crate::gifs::LOCALE
        );
        self.json(self.request(Method::GET, &path)).await
    }

    /// The GIFs trending now, for the picker's "Trending GIFs".
    pub async fn gifs_trending_gifs(&self) -> Result<Value> {
        let path = format!(
            "/gifs/trending-gifs?media_format={}&locale={}",
            crate::gifs::MEDIA_FORMAT,
            crate::gifs::LOCALE
        );
        self.json(self.request(Method::GET, &path)).await
    }

    /// GIFs for `query`, as many as Discord gives at once.
    pub async fn gifs_search(&self, query: &str) -> Result<Value> {
        let path = format!(
            "/gifs/search?q={}&media_format={}&locale={}",
            encode(query),
            crate::gifs::MEDIA_FORMAT,
            crate::gifs::LOCALE
        );
        self.json(self.request(Method::GET, &path)).await
    }

    /// Terms like `query` to search for instead.
    pub async fn gifs_suggest(&self, query: &str, limit: u32) -> Result<Value> {
        let path = format!(
            "/gifs/suggest?q={}&limit={limit}&locale={}",
            encode(query),
            crate::gifs::LOCALE
        );
        self.json(self.request(Method::GET, &path)).await
    }

    /// Tells the provider which GIF a search for `query` led to.
    pub async fn gif_select(&self, id: &str, query: &str) -> Result<()> {
        let body = json!({"id": id, "q": query});
        self.empty(with_json(self.request(Method::POST, "/gifs/select"), &body))
            .await
    }

    /// The account's second settings, `FrecencyUserSettings`, in base64.
    pub async fn frecency_settings(&self) -> Result<String> {
        let answer = self
            .json(self.request(Method::GET, "/users/@me/settings-proto/2"))
            .await?;
        Ok(answer["settings"].as_str().unwrap_or_default().to_owned())
    }

    /// Replaces the fields `settings`, base64 `FrecencyUserSettings`, holds.
    pub async fn set_frecency_settings(&self, settings: &str) -> Result<()> {
        let body = json!({"settings": settings});
        self.json(with_json(
            self.request(Method::PATCH, "/users/@me/settings-proto/2"),
            &body,
        ))
        .await
        .map(drop)
    }

    /// Sets the status: `online`, `idle`, `dnd` or `invisible`.
    pub async fn set_status(&self, status: &str) -> Result<()> {
        let body = json!({"settings": settings::write_status(status)});
        self.json(with_json(
            self.request(Method::PATCH, "/users/@me/settings-proto/1"),
            &body,
        ))
        .await
        .map(drop)
    }

    /// Ignores a friend request, cancels one, or ends a friendship.
    pub async fn remove_relationship(&self, user: &str, place: &str) -> Result<()> {
        let path = format!("/users/@me/relationships/{user}");
        let request = self.request(Method::DELETE, &path);
        self.empty(request.header("X-Context-Properties", context(place)))
            .await
    }

    /// Blocks `user`, which ends a friendship or friend request with them.
    pub async fn block(&self, user: &str, place: &str) -> Result<()> {
        let path = format!("/users/@me/relationships/{user}");
        let request = with_json(self.request(Method::PUT, &path), &json!({"type": 2}));
        self.empty(request.header("X-Context-Properties", context(place)))
            .await
    }

    /// Ignores `user`, or stops ignoring them: Discord hides their messages
    /// and quiets their DMs, and they aren't told.
    pub async fn ignore(&self, user: &str, ignore: bool) -> Result<()> {
        let path = format!("/users/@me/relationships/{user}/ignore");
        let method = if ignore { Method::PUT } else { Method::DELETE };
        self.empty(self.request(method, &path)).await
    }

    /// Sets the nickname this account gives a friend, or clears it with an
    /// empty one.
    pub async fn set_friend_nickname(&self, user: &str, nickname: &str) -> Result<()> {
        let path = format!("/users/@me/relationships/{user}");
        let nickname = (!nickname.is_empty()).then_some(nickname);
        let body = json!({"nickname": nickname});
        self.empty(with_json(self.request(Method::PATCH, &path), &body))
            .await
    }

    /// This account's note on `user`; empty when it has none.
    pub async fn note(&self, user: &str) -> Result<String> {
        let path = format!("/users/@me/notes/{user}");
        match self.json(self.request(Method::GET, &path)).await {
            Ok(answer) => Ok(answer["note"].as_str().unwrap_or_default().to_owned()),
            // Discord keeps no empty notes.
            Err(error)
                if matches!(
                    error.downcast_ref::<Refused>(),
                    Some(Refused(StatusCode::NOT_FOUND))
                ) =>
            {
                Ok(String::new())
            }
            Err(error) => Err(error),
        }
    }

    /// Sets this account's note on `user`, or removes it with an empty one.
    pub async fn set_note(&self, user: &str, note: &str) -> Result<()> {
        let path = format!("/users/@me/notes/{user}");
        let body = json!({"note": note});
        self.empty(with_json(self.request(Method::PUT, &path), &body))
            .await
    }

    /// Closes a DM, which leaves the DM list until either side writes again.
    pub async fn close_dm(&self, channel: &str) -> Result<()> {
        let path = format!("/channels/{channel}");
        self.json(self.request(Method::DELETE, &path))
            .await
            .map(drop)
    }

    /// Changes this account's notification settings for a server, or with
    /// [`crate::ready::PRIVATE`] its DMs, by `body`, a partial settings
    /// object; see [`crate::notifications::Change::body`]. Discord answers
    /// with USER_GUILD_SETTINGS_UPDATE too.
    pub async fn guild_settings(&self, guild: &str, body: &Value) -> Result<()> {
        let path = format!("/users/@me/guilds/{guild}/settings");
        self.json(with_json(self.request(Method::PATCH, &path), body))
            .await
            .map(drop)
    }

    /// Changes this account's settings for one or more servers, never its
    /// DMs, by `body`, `{"guilds": {"<id>": <partial settings>}}`, as
    /// Discord's client does; see [`crate::notifications::flags_body`].
    /// Discord answers with USER_GUILD_SETTINGS_UPDATE for each server.
    pub async fn guilds_settings(&self, body: &Value) -> Result<()> {
        self.json(with_json(
            self.request(Method::PATCH, "/users/@me/guilds/settings"),
            body,
        ))
        .await
        .map(drop)
    }

    // A server member's menu: nicknames, roles and moderation. Each fires
    // GUILD_MEMBER_UPDATE, or for a kick GUILD_MEMBER_REMOVE and a ban
    // GUILD_BAN_ADD; see <https://docs.discord.food/resources/guild>.

    /// Sets the nickname of `user` in a server, or with none this account's
    /// own, which takes Change Nickname rather than Manage Nicknames; empty
    /// resets it.
    pub async fn set_member_nick(&self, guild: &str, user: Option<&str>, nick: &str) -> Result<()> {
        let path = format!("/guilds/{guild}/members/{}", user.unwrap_or("@me"));
        let body = json!({"nick": (!nick.is_empty()).then_some(nick)});
        self.empty(with_json(self.request(Method::PATCH, &path), &body))
            .await
    }

    /// Changes this account's member object in a server: its nickname and
    /// server avatar, as Discord's per-server profile editor sends them.
    /// Discord answers with the member, and GUILD_MEMBER_UPDATE.
    pub async fn modify_member_me(&self, guild: &str, body: &Value) -> Result<Value> {
        let path = format!("/guilds/{guild}/members/@me");
        self.json(with_json(self.request(Method::PATCH, &path), body))
            .await
    }

    /// Changes this account's profile in a server: its banner, bio and
    /// pronouns. Discord answers with the profile, and no gateway event.
    pub async fn modify_guild_profile(&self, guild: &str, body: &Value) -> Result<Value> {
        let path = format!("/guilds/{guild}/profile/@me");
        self.json(with_json(self.request(Method::PATCH, &path), body))
            .await
    }

    /// Changes a member's voice state, as the voice menu does: `mute` and
    /// `deaf` for the server's, and `channel_id` to move them, which with
    /// null disconnects them. Discord answers with the member, and
    /// VOICE_STATE_UPDATE.
    pub async fn modify_member_voice(&self, guild: &str, user: &str, body: &Value) -> Result<()> {
        let path = format!("/guilds/{guild}/members/{user}");
        self.empty(with_json(self.request(Method::PATCH, &path), body))
            .await
    }

    /// Gives a member a role, or with `add` false takes it away.
    pub async fn member_role(&self, guild: &str, user: &str, role: &str, add: bool) -> Result<()> {
        let path = format!("/guilds/{guild}/members/{user}/roles/{role}");
        let method = if add { Method::PUT } else { Method::DELETE };
        self.empty(self.request(method, &path)).await
    }

    /// Times a member out until `until`, an ISO 8601 time at most 28 days
    /// away, or with none ends their timeout.
    pub async fn time_out(
        &self,
        guild: &str,
        user: &str,
        until: Option<&str>,
        reason: &str,
    ) -> Result<()> {
        let path = format!("/guilds/{guild}/members/{user}");
        let body = json!({"communication_disabled_until": until});
        let request = with_json(self.request(Method::PATCH, &path), &body);
        self.empty(audit_reason(request, reason)).await
    }

    /// Removes a member from a server; they may join again with an invite.
    pub async fn kick(&self, guild: &str, user: &str, reason: &str) -> Result<()> {
        let path = format!("/guilds/{guild}/members/{user}");
        self.empty(audit_reason(self.request(Method::DELETE, &path), reason))
            .await
    }

    /// Bans a member, deleting what they wrote in the last `delete_seconds`,
    /// up to a week.
    pub async fn ban(
        &self,
        guild: &str,
        user: &str,
        delete_seconds: u32,
        reason: &str,
    ) -> Result<()> {
        let path = format!("/guilds/{guild}/bans/{user}");
        let body = json!({"delete_message_seconds": delete_seconds.min(MAX_BAN_DELETE)});
        let request = with_json(self.request(Method::PUT, &path), &body);
        self.empty(audit_reason(request, reason)).await
    }

    /// Makes an invite to a server's channel that lasts `max_age` seconds,
    /// 0 for good, and works `max_uses` times, 0 for any number, as
    /// Discord's invite dialog asks; its code.
    pub async fn create_invite(
        &self,
        channel: &str,
        max_age: u32,
        max_uses: u32,
    ) -> Result<String> {
        let path = format!("/channels/{channel}/invites");
        let body = json!({"max_age": max_age, "max_uses": max_uses, "temporary": false});
        self.invite(with_json(self.request(Method::POST, &path), &body))
            .await
    }

    /// Makes an invite to a server's channel for one person, as Discord's
    /// Invite to Server does: it works once and lasts Discord's default of
    /// a day, and `unique` asks for a new one rather than a similar one
    /// reused; its code.
    pub async fn create_invite_once(&self, channel: &str) -> Result<String> {
        let path = format!("/channels/{channel}/invites");
        let body = json!({"max_uses": 1, "unique": true});
        let request = with_json(self.request(Method::POST, &path), &body)
            .header("X-Context-Properties", context("User Invite Context Menu"));
        self.invite(request).await
    }

    /// The code of the invite a request made.
    async fn invite(&self, request: RequestBuilder) -> Result<String> {
        let response = self.execute(request).await?;
        // Discord answers 204, with nothing, when the server paused invites.
        if response.status() == StatusCode::NO_CONTENT {
            bail!("This server has paused invites.");
        }
        let invite: Value = serde_json::from_slice(
            &response
                .bytes()
                .await
                .context("could not read Discord's answer")?,
        )
        .context("could not understand Discord's answer")?;
        invite["code"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("Discord made no invite."))
    }

    /// A channel object, as `GET /channels/{id}` gives it.
    pub async fn channel(&self, channel: &str) -> Result<Value> {
        let path = format!("/channels/{channel}");
        self.json(self.request(Method::GET, &path)).await
    }

    /// Makes a channel or category in a server from `body`, a channel
    /// object's fields; the new channel object. Discord sends CHANNEL_CREATE.
    pub async fn create_channel(&self, guild: &str, body: &Value) -> Result<Value> {
        let path = format!("/guilds/{guild}/channels");
        self.json(with_json(self.request(Method::POST, &path), body))
            .await
    }

    /// Changes a channel's or category's fields that `body` names; Discord
    /// sends CHANNEL_UPDATE.
    pub async fn edit_channel(&self, channel: &str, body: &Value) -> Result<()> {
        let path = format!("/channels/{channel}");
        self.json(with_json(self.request(Method::PATCH, &path), body))
            .await
            .map(drop)
    }

    /// Deletes a server's channel or category for good; Discord sends
    /// CHANNEL_DELETE, and moves a category's channels out of it.
    pub async fn delete_channel(&self, channel: &str) -> Result<()> {
        let path = format!("/channels/{channel}");
        self.json(self.request(Method::DELETE, &path))
            .await
            .map(drop)
    }

    /// Leaves a server, as a member rather than one lurking in it.
    pub async fn leave_guild(&self, guild: &str) -> Result<()> {
        let path = format!("/users/@me/guilds/{guild}");
        self.empty(with_json(
            self.request(Method::DELETE, &path),
            &json!({"lurking": false}),
        ))
        .await
    }

    /// Whether this account may ring the others in a DM, beyond calling them.
    pub async fn ringable(&self, channel: &str) -> Result<bool> {
        let path = format!("/channels/{channel}/call");
        let answer = self.json(self.request(Method::GET, &path)).await?;
        Ok(answer["ringable"].as_bool().unwrap_or(false))
    }

    /// Rings everyone in a DM or group DM call who isn't in it yet.
    pub async fn ring(&self, channel: &str) -> Result<()> {
        let path = format!("/channels/{channel}/call/ring");
        let body = json!({"recipients": null});
        self.empty(with_json(self.request(Method::POST, &path), &body))
            .await
    }

    /// Stops ringing this account for a call, which declines it.
    pub async fn stop_ringing(&self, channel: &str, me: &str) -> Result<()> {
        let path = format!("/channels/{channel}/call/stop-ringing");
        let body = json!({"recipients": [me]});
        self.empty(with_json(self.request(Method::POST, &path), &body))
            .await
    }

    /// Shows `thumbnail`, a JPEG as a data URI, as the preview of this
    /// account's live stream, as Discord's client does every few minutes.
    pub async fn stream_preview(&self, stream_key: &str, thumbnail: &str) -> Result<()> {
        let path = format!("/streams/{stream_key}/preview");
        let body = json!({"thumbnail": thumbnail});
        self.empty(with_json(self.request(Method::POST, &path), &body))
            .await
    }

    /// Downloads a file from Discord's CDN or media proxy, or the test
    /// server, refusing anything over `limit` bytes.
    pub async fn download(&self, url: &str, limit: usize) -> Result<Vec<u8>> {
        let test_file = client::test_server()
            .and_then(|server| url.strip_prefix(server))
            .is_some_and(|path| path.starts_with('/'));
        if !discord_file(url) && !test_file && !crate::gifs::provider_media(url) {
            bail!("not a Discord file");
        }
        let _turn = self.downloads.acquire().await?;
        let mut response = self
            .http
            .get(url)
            .send()
            .await
            .context("could not reach Discord")?;
        if !response.status().is_success() {
            return Err(Refused(response.status()).into());
        }
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await.context("the download stopped")? {
            data.extend_from_slice(&chunk);
            if data.len() > limit {
                bail!("the file is too large");
            }
        }
        Ok(data)
    }

    /// Downloads a file from Discord's CDN or media proxy into `file` as it
    /// arrives, as [`Self::download`] does, returning its size.
    pub async fn download_into(
        &self,
        url: &str,
        file: &mut std::fs::File,
        limit: u64,
    ) -> Result<u64> {
        use std::io::Write;
        let test_file = client::test_server()
            .and_then(|server| url.strip_prefix(server))
            .is_some_and(|path| path.starts_with('/'));
        if !discord_file(url) && !test_file && !crate::gifs::provider_media(url) {
            bail!("not a Discord file");
        }
        let mut response = self
            .http
            .get(url)
            .send()
            .await
            .context("could not reach Discord")?;
        if !response.status().is_success() {
            return Err(Refused(response.status()).into());
        }
        let mut size = 0;
        while let Some(chunk) = response.chunk().await.context("the download stopped")? {
            size += chunk.len() as u64;
            if size > limit {
                bail!("the file is too large");
            }
            file.write_all(&chunk).context("could not keep the file")?;
        }
        file.flush().context("could not keep the file")?;
        Ok(size)
    }

    async fn json(&self, request: RequestBuilder) -> Result<Value> {
        let response = self.execute(request).await?;
        let body = response
            .bytes()
            .await
            .context("could not read Discord's answer")?;
        serde_json::from_slice(&body).context("could not understand Discord's answer")
    }

    async fn empty(&self, request: RequestBuilder) -> Result<()> {
        self.execute(request).await.map(drop)
    }

    /// Sends a request within Discord's rate limits; see [`crate::limits`].
    /// A 429 means Discord didn't act on it, so it goes once more after the
    /// wait Discord asks for.
    async fn execute(&self, request: RequestBuilder) -> Result<Response> {
        let (client, request) = request.build_split();
        let mut request = request.context("could not build the request")?;
        let route = limits::route(request.method(), request.url().path());
        let mut retried = false;
        loop {
            match self.refusals.check() {
                Ok(()) => {}
                Err(Stopped::Unauthorized) => bail!(UNAUTHORIZED),
                Err(Stopped::Refused(wait)) => bail!(
                    "Discord refused too many of Yapfast's requests lately, so Yapfast stopped asking. Try again in {} min.",
                    wait.as_secs().div_ceil(60)
                ),
            }
            if let Err(wait) = self.limits.acquire(&route).await {
                bail!(
                    "Discord asks Yapfast to slow down. Try again in {} s.",
                    wait.as_secs()
                );
            }
            let again = if retried { None } else { request.try_clone() };
            let response = client
                .execute(request)
                .await
                .context("Can't reach Discord.")?;
            let status = response.status();
            let linked = !self.token().is_empty();
            if linked {
                self.refusals.answered(status);
            }
            if status == StatusCode::UNAUTHORIZED && linked {
                let body = body(response).await;
                // Discord's answer to a login it no longer takes, rather than
                // to one request.
                if body["code"] == 0 {
                    tracing::warn!("Discord refused the login; Yapfast stops asking");
                    self.refusals.unauthorized();
                }
                return Err(refusal(status, &body));
            }
            if status != StatusCode::TOO_MANY_REQUESTS {
                self.limits.update(&route, response.headers());
                return checked(response).await;
            }
            let headers = response.headers().clone();
            let body = body(response).await;
            self.limits.limited(&route, &headers, &body);
            let Some(next) = again else {
                return Err(refusal(status, &body));
            };
            tracing::warn!(
                route,
                "Discord limited a request; sending it again after the wait"
            );
            request = next;
            retried = true;
        }
    }
}

/// Turns Discord's refusals into errors a person can act on.
async fn checked(response: Response) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(refusal(status, &body(response).await))
}

/// An answer's JSON body, or null.
async fn body(response: Response) -> Value {
    match response.bytes().await {
        Ok(body) => serde_json::from_slice(&body).unwrap_or_default(),
        Err(_) => Value::Null,
    }
}

/// A refusal with this status and body, as an error a person can act on.
fn refusal(status: StatusCode, body: &Value) -> anyhow::Error {
    // Discord's own words go in the error's chain, for logs: `{:#}` shows them.
    // The status stays at its root, for callers to tell refusals apart.
    let detail = anyhow::Error::new(Refused(status));
    let detail = match (body["code"].as_u64(), body["message"].as_str()) {
        (Some(code), Some(message)) => detail.context(format!("code {code}: {message}")),
        _ => detail,
    };
    detail.context(match (status, body["code"].as_u64()) {
        (_, Some(UNKNOWN_USERNAME)) => anyhow!("Discord has nobody by that username."),
        (_, Some(REQUESTS_CLOSED)) => {
            anyhow!("They don't take friend requests from people they don't know.")
        }
        (_, Some(ALREADY_FRIENDS)) => anyhow!("You're already friends."),
        _ => match status {
            StatusCode::UNAUTHORIZED => anyhow!(UNAUTHORIZED),
            StatusCode::FORBIDDEN => {
                anyhow!("Discord says you don't have permission to do that here.")
            }
            StatusCode::NOT_FOUND => anyhow!("Discord no longer has that message or channel."),
            StatusCode::PAYLOAD_TOO_LARGE => {
                anyhow!("Discord says the file is too large for your account.")
            }
            StatusCode::TOO_MANY_REQUESTS => {
                let wait = body["retry_after"].as_f64().unwrap_or(1.0).ceil();
                anyhow!("Discord asks Yapfast to slow down. Try again in {wait} s.")
            }
            StatusCode::BAD_REQUEST if body["captcha_key"].is_array() => {
                let message = "Discord asked for a captcha. Do this in the Discord app for now.";
                // The captcha, for a caller that can show it.
                return match Captcha::from_body(body) {
                    Some(captcha) => anyhow::Error::new(captcha).context(message),
                    None => anyhow!(message),
                };
            }
            _ => match body["message"].as_str() {
                Some(message) => anyhow!("Discord refused: {message}"),
                None => anyhow!("Discord answered {status}."),
            },
        },
    })
}

/// Discord's error codes that Yapfast words itself.
const REQUESTS_CLOSED: u64 = 80000;
const UNKNOWN_USERNAME: u64 = 80004;
const ALREADY_FRIENDS: u64 = 80007;

/// The X-Context-Properties header Discord's client sends: where an action
/// happened, as base64 JSON.
fn context(place: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(json!({"location": place}).to_string())
}

/// A new message's body as Discord's client sends it, before attachments
/// and a reply.
fn message_body(content: &str, nonce: &str) -> Value {
    json!({
        "mobile_network_type": "unknown",
        "content": content,
        "nonce": nonce,
        "tts": false,
        "flags": 0,
    })
}

fn with_json(request: RequestBuilder, body: &Value) -> RequestBuilder {
    request
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.to_string())
}

/// Percent-encodes a path segment.
/// The most of a banned member's messages a ban deletes: a week.
pub const MAX_BAN_DELETE: u32 = 7 * 24 * 3600;

/// Discord keeps up to 512 characters of an audit log's reason.
const MAX_REASON: usize = 512;

/// Gives a moderation request the reason the audit log shows, URL-encoded as
/// Discord reads it; none when it's empty.
fn audit_reason(request: RequestBuilder, reason: &str) -> RequestBuilder {
    let reason = reason.trim();
    if reason.is_empty() {
        return request;
    }
    let end = reason
        .char_indices()
        .nth(MAX_REASON)
        .map_or(reason.len(), |(end, _)| end);
    request.header("X-Audit-Log-Reason", encode(&reason[..end]))
}

fn encode(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len() * 3);
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Whether `url` is on Discord's CDN or its media proxy, which serves
/// embeds' pictures from `images-ext-1.discordapp.net` and the like.
fn discord_file(url: &str) -> bool {
    let Some(host) = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split_once('/'))
        .map(|(host, _)| host)
    else {
        return false;
    };
    let proxy = host
        .strip_prefix("images-ext-")
        .and_then(|host| host.strip_suffix(".discordapp.net"))
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()));
    proxy || host == "cdn.discordapp.com" || host == "media.discordapp.net"
}

#[cfg(test)]
mod tests {
    #[test]
    fn downloads_that_may_work_later() {
        use super::{Refused, StatusCode, may_work_later};
        let refused = |status| anyhow::Error::from(Refused(status));
        assert!(may_work_later(&refused(StatusCode::SERVICE_UNAVAILABLE)));
        assert!(may_work_later(&refused(StatusCode::TOO_MANY_REQUESTS)));
        assert!(!may_work_later(&refused(StatusCode::NOT_FOUND)));
        assert!(!may_work_later(&refused(StatusCode::FORBIDDEN)));
        assert!(!may_work_later(&anyhow::anyhow!("the file is too large")));
    }

    #[test]
    fn upload_limits_follow_nitro_and_boosts() {
        use super::{MIB, upload_limit};
        assert_eq!(upload_limit(0, None), 20 * MIB, "without Nitro");
        assert_eq!(upload_limit(3, None), 50 * MIB, "Nitro Basic");
        assert_eq!(upload_limit(2, None), 500 * MIB, "Nitro");
        assert_eq!(upload_limit(0, Some(1)), 20 * MIB, "level 1 adds nothing");
        assert_eq!(upload_limit(0, Some(2)), 50 * MIB);
        assert_eq!(upload_limit(0, Some(3)), 100 * MIB);
        assert_eq!(upload_limit(2, Some(3)), 500 * MIB, "the larger wins");
    }

    #[test]
    fn says_where_actions_happen_as_discords_client_does() {
        // The values Discord's client sends, as discord.py-self records them.
        assert_eq!(
            super::context("User Profile"),
            "eyJsb2NhdGlvbiI6IlVzZXIgUHJvZmlsZSJ9"
        );
        assert_eq!(
            super::context("Friends"),
            "eyJsb2NhdGlvbiI6IkZyaWVuZHMifQ=="
        );
    }

    #[test]
    fn encodes_emoji_for_paths() {
        assert_eq!(super::encode("👍"), "%F0%9F%91%8D");
        assert_eq!(super::encode("pog:5"), "pog%3A5");
    }

    #[test]
    fn downloads_only_from_discord() {
        assert!(super::discord_file(
            "https://cdn.discordapp.com/attachments/1/2/a.png"
        ));
        assert!(super::discord_file(
            "https://images-ext-2.discordapp.net/external/x/https/example.com/a.png"
        ));
        assert!(!super::discord_file(
            "https://images-ext-.discordapp.net/a.png"
        ));
        assert!(!super::discord_file("https://example.com/a.png"));
        assert!(!super::discord_file(
            "https://cdn.discordapp.com.example.com/a.png"
        ));
    }
}
