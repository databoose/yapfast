//! Discord's HTTP API, called the way the web client calls it.

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, header};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    client,
    limits::{self, Limits},
    settings::{self, CustomStatus},
};

const API: &str = "https://discord.com/api/v9";
/// The most messages one history request returns.
pub const PAGE: usize = 50;

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

const MIB: u64 = 1024 * 1024;

/// The largest file this account may send, in bytes: by its Nitro, as
/// [`crate::service::Account::premium_type`], or in a server boosted to
/// `premium_tier` 2 or 3, by the server's, whichever allows more. These are
/// Discord's limits since August 2026.
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
/// out of reach, failed or asked Talk to slow down, rather than refusing the
/// file or Talk refusing what came.
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
    token: String,
    /// The API's base URL: Discord's, or the test server's.
    base: String,
    limits: Limits,
    /// Downloads going at once, so opening the emoji picker or a busy channel
    /// spreads its pictures out rather than asking for hundreds together.
    downloads: tokio::sync::Semaphore,
}

/// The most files Talk downloads from Discord at once. They share one HTTP/2
/// connection, as a browser's requests to one host do.
const DOWNLOADS: usize = 8;

impl Api {
    pub fn new(token: String) -> Result<Self> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "X-Super-Properties",
            header::HeaderValue::from_str(&client::super_properties())?,
        );
        let http = Client::builder()
            .user_agent(client::USER_AGENT)
            .default_headers(headers)
            .cookie_provider(crate::cookies::jar())
            .build()
            .context("could not set up HTTPS")?;
        let base = match client::test_server() {
            Some(server) => format!("{server}/api/v9"),
            None => API.to_owned(),
        };
        Ok(Self {
            http,
            token,
            base,
            limits: Limits::default(),
            downloads: tokio::sync::Semaphore::new(DOWNLOADS),
        })
    }

    /// Without a login, for linking one.
    pub fn anonymous() -> Result<Self> {
        Self::new(String::new())
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let request = self.http.request(method, format!("{}{path}", self.base));
        if self.token.is_empty() {
            return request;
        }
        request.header(header::AUTHORIZATION, &self.token)
    }

    /// Trades a remote login's ticket for the login, encrypted with the key
    /// that started it, in base64; see [`crate::remote_auth`].
    pub async fn remote_auth_login(&self, ticket: &str) -> Result<String> {
        let body = json!({"ticket": ticket});
        let answer = self
            .json(with_json(
                self.request(Method::POST, "/users/@me/remote-auth/login"),
                &body,
            ))
            .await?;
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

    /// Up to [`PAGE`] messages before `before`, or the latest, newest first.
    pub async fn messages(&self, channel: &str, before: Option<&str>) -> Result<Vec<Value>> {
        let mut path = format!("/channels/{channel}/messages?limit={PAGE}");
        if let Some(before) = before {
            path.push_str(&format!("&before={before}"));
        }
        match self.json(self.request(Method::GET, &path)).await? {
            Value::Array(messages) => Ok(messages),
            _ => bail!("could not understand Discord's answer"),
        }
    }

    /// Sends a message, as a reply when `reply` names one in the same
    /// channel, with an uploaded file.
    pub async fn send(
        &self,
        guild: Option<&str>,
        channel: &str,
        content: &str,
        nonce: &str,
        reply: Option<&str>,
        uploads: &[Upload],
    ) -> Result<Value> {
        let mut body = json!({
            "mobile_network_type": "unknown",
            "content": content,
            "nonce": nonce,
            "tts": false,
            "flags": 0,
        });
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
        if let Some(reply) = reply {
            body["message_reference"] = json!({
                "guild_id": guild,
                "channel_id": channel,
                "message_id": reply,
                "type": 0,
            });
        }
        let path = format!("/channels/{channel}/messages");
        self.json(with_json(self.request(Method::POST, &path), &body))
            .await
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

    pub async fn delete(&self, channel: &str, message: &str) -> Result<()> {
        let path = format!("/channels/{channel}/messages/{message}");
        self.empty(self.request(Method::DELETE, &path)).await
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
        if !discord_file(url) && !test_file {
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
        if !discord_file(url) && !test_file {
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
            if let Err(wait) = self.limits.acquire(&route).await {
                bail!(
                    "Discord asks Talk to slow down. Try again in {} s.",
                    wait.as_secs()
                );
            }
            let again = if retried { None } else { request.try_clone() };
            let response = client
                .execute(request)
                .await
                .context("Can't reach Discord.")?;
            let status = response.status();
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
    let detail = match (body["code"].as_u64(), body["message"].as_str()) {
        (Some(code), Some(message)) => anyhow!("Discord answered {status}, code {code}: {message}"),
        _ => anyhow!("Discord answered {status}"),
    };
    detail.context(match (status, body["code"].as_u64()) {
        (_, Some(UNKNOWN_USERNAME)) => anyhow!("Discord has nobody by that username."),
        (_, Some(REQUESTS_CLOSED)) => {
            anyhow!("They don't take friend requests from people they don't know.")
        }
        (_, Some(ALREADY_FRIENDS)) => anyhow!("You're already friends."),
        _ => match status {
            StatusCode::UNAUTHORIZED => anyhow!("Discord rejected the login. Link Discord again."),
            StatusCode::FORBIDDEN => {
                anyhow!("Discord says you don't have permission to do that here.")
            }
            StatusCode::NOT_FOUND => anyhow!("Discord no longer has that message or channel."),
            StatusCode::PAYLOAD_TOO_LARGE => {
                anyhow!("Discord says the file is too large for your account.")
            }
            StatusCode::TOO_MANY_REQUESTS => {
                let wait = body["retry_after"].as_f64().unwrap_or(1.0).ceil();
                anyhow!("Discord asks Talk to slow down. Try again in {wait} s.")
            }
            _ if body["captcha_key"].is_array() => {
                anyhow!("Discord asked for a captcha. Do this in the Discord app for now.")
            }
            _ => match body["message"].as_str() {
                Some(message) => anyhow!("Discord refused: {message}"),
                None => anyhow!("Discord answered {status}."),
            },
        },
    })
}

/// Discord's error codes that Talk words itself.
const REQUESTS_CLOSED: u64 = 80000;
const UNKNOWN_USERNAME: u64 = 80004;
const ALREADY_FRIENDS: u64 = 80007;

/// The X-Context-Properties header Discord's client sends: where an action
/// happened, as base64 JSON.
fn context(place: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(json!({"location": place}).to_string())
}

fn with_json(request: RequestBuilder, body: &Value) -> RequestBuilder {
    request
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.to_string())
}

/// Percent-encodes a path segment.
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
