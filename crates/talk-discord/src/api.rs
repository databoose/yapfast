//! Discord's HTTP API, called the way the web client calls it.

use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode, header};
use serde::Deserialize;

use crate::client;

const API: &str = "https://discord.com/api/v9";

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
}

pub struct Api {
    http: Client,
    token: String,
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
        Ok(Self { http, token })
    }

    /// The account the token belongs to.
    pub async fn me(&self) -> Result<User> {
        let response = self
            .http
            .get(format!("{API}/users/@me"))
            .header(header::AUTHORIZATION, &self.token)
            .send()
            .await
            .context("could not reach Discord")?;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::UNAUTHORIZED => {
                bail!("Discord rejected the token; it may have been logged out")
            }
            status => bail!("Discord answered {status}"),
        }
        let body = response
            .bytes()
            .await
            .context("could not read Discord's answer")?;
        serde_json::from_slice(&body).context("could not understand Discord's answer")
    }
}
