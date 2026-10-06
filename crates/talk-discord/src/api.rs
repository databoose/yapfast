//! Discord's HTTP API, called the way the web client calls it.

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, header};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client;

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

pub struct Api {
    http: Client,
    token: String,
    /// The API's base URL: Discord's, or the test server's.
    base: String,
}

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
            .build()
            .context("could not set up HTTPS")?;
        let base = match client::test_server() {
            Some(server) => format!("{server}/api/v9"),
            None => API.to_owned(),
        };
        Ok(Self { http, token, base })
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .header(header::AUTHORIZATION, &self.token)
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
        upload: Option<&Upload>,
    ) -> Result<Value> {
        let mut body = json!({
            "mobile_network_type": "unknown",
            "content": content,
            "nonce": nonce,
            "tts": false,
            "flags": 0,
        });
        if let Some(upload) = upload {
            body["attachments"] = json!([{
                "id": "0",
                "filename": upload.filename,
                "uploaded_filename": upload.uploaded_filename,
            }]);
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
        let mut response = self
            .http
            .get(url)
            .send()
            .await
            .context("could not reach Discord")?;
        if !response.status().is_success() {
            bail!("Discord answered {}", response.status());
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

    async fn json(&self, request: RequestBuilder) -> Result<Value> {
        let response = checked(request).await?;
        let body = response
            .bytes()
            .await
            .context("could not read Discord's answer")?;
        serde_json::from_slice(&body).context("could not understand Discord's answer")
    }

    async fn empty(&self, request: RequestBuilder) -> Result<()> {
        checked(request).await.map(drop)
    }
}

/// Sends a request and turns Discord's refusals into errors a person can act on.
async fn checked(request: RequestBuilder) -> Result<Response> {
    let response = request.send().await.context("Can't reach Discord.")?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body: Value = match response.bytes().await {
        Ok(body) => serde_json::from_slice(&body).unwrap_or_default(),
        Err(_) => Value::Null,
    };
    Err(match status {
        StatusCode::UNAUTHORIZED => anyhow!("Discord rejected the login. Link Discord again."),
        StatusCode::FORBIDDEN => anyhow!("Discord says you don't have permission to do that here."),
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
    })
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
