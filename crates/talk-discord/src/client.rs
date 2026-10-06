//! How Talk presents itself to Discord. The gateway and HTTP API both carry the
//! same description, matching a current desktop browser on the web client.

use std::sync::OnceLock;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

/// A local fake Discord for tests and measurements, such as
/// `http://127.0.0.1:18400`; see testing/fake_discord.ts. While it is set,
/// Talk talks only to it, with [`TEST_TOKEN`] instead of the saved login.
pub fn test_server() -> Option<&'static str> {
    static SERVER: OnceLock<Option<String>> = OnceLock::new();
    SERVER
        .get_or_init(|| {
            let server = std::env::var("TALK_DISCORD_TEST_SERVER").ok()?;
            let server = server.trim().trim_end_matches('/');
            (!server.is_empty()).then(|| server.to_owned())
        })
        .as_deref()
}

/// The token Talk uses with the test server.
pub const TEST_TOKEN: &str = "talk-test-token";

pub const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:143.0) Gecko/20100101 Firefox/143.0";
const BROWSER_VERSION: &str = "143.0";
/// The web client's build number. mautrix-discord scrapes the current one from
/// discord.com and falls back to this value; Talk will do the same later.
const CLIENT_BUILD_NUMBER: u64 = 497254;

/// The `properties` object sent when identifying on the gateway.
pub fn properties() -> Value {
    json!({
        "os": "Linux",
        "browser": "Firefox",
        "device": "",
        "system_locale": "en-US",
        "browser_user_agent": USER_AGENT,
        "browser_version": BROWSER_VERSION,
        "os_version": "",
        "referrer": "",
        "referring_domain": "",
        "referrer_current": "",
        "referring_domain_current": "",
        "release_channel": "stable",
        "client_build_number": CLIENT_BUILD_NUMBER,
        "client_event_source": null,
    })
}

/// The same properties for HTTP requests' `X-Super-Properties` header.
pub fn super_properties() -> String {
    STANDARD.encode(properties().to_string())
}
