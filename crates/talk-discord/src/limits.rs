//! Discord's HTTP rate limits, which its API asks every client to keep. Each
//! route belongs to a bucket, named in `X-RateLimit-Bucket`, that allows
//! `X-RateLimit-Remaining` more requests until `X-RateLimit-Reset-After`
//! seconds pass, and a 429 marked global pauses every route. Routes about
//! different channels, servers or webhooks share a bucket's name but not its
//! count. Talk waits rather than sending a request Discord would refuse:
//! refusals count against the account, and too many of them get its address
//! blocked for a while.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use reqwest::{Method, StatusCode, header::HeaderMap};
use serde_json::Value;

/// The longest Talk waits on a limit before giving up on the request.
pub const MAX_WAIT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct Limits {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The bucket each route counts against, as Discord last said: its name
    /// and the route's channel, server or webhook.
    buckets_by_route: HashMap<String, String>,
    buckets: HashMap<String, Bucket>,
    /// Every route waits until then.
    global_until: Option<Instant>,
}

struct Bucket {
    remaining: u64,
    reset: Instant,
}

impl Limits {
    /// Waits until a request on `route` may go, and counts it against its
    /// bucket. Errors when that would take longer than [`MAX_WAIT`].
    pub async fn acquire(&self, route: &str) -> Result<(), Duration> {
        loop {
            let wait = self.reserve(route, Instant::now());
            if wait.is_zero() {
                return Ok(());
            }
            if wait > MAX_WAIT {
                return Err(wait);
            }
            tokio::time::sleep(wait).await;
        }
    }

    /// How long a request on `route` must wait; when it needn't, the request
    /// takes one of its bucket's remaining requests.
    fn reserve(&self, route: &str, now: Instant) -> Duration {
        let mut inner = self.lock();
        if let Some(until) = inner.global_until {
            if until > now {
                return until - now;
            }
            inner.global_until = None;
        }
        let Some(key) = inner.buckets_by_route.get(route).cloned() else {
            return Duration::ZERO;
        };
        let Some(bucket) = inner.buckets.get_mut(&key) else {
            return Duration::ZERO;
        };
        if bucket.reset <= now {
            // The bucket refilled; Discord's next answer says by how much.
            inner.buckets.remove(&key);
            return Duration::ZERO;
        }
        if bucket.remaining == 0 {
            return bucket.reset - now;
        }
        bucket.remaining -= 1;
        Duration::ZERO
    }

    /// Learns the limits from an answer to a request on `route` that
    /// Discord didn't refuse with a 429; see [`Self::limited`] for those.
    pub fn update(&self, route: &str, headers: &HeaderMap) {
        self.update_at(route, StatusCode::OK, headers, &Value::Null, Instant::now());
    }

    /// Learns the limits from a 429 on `route`, with its JSON `body`, whose
    /// `retry_after` and `global` say the same as the headers, or say it
    /// when the headers don't.
    pub fn limited(&self, route: &str, headers: &HeaderMap, body: &Value) {
        let status = StatusCode::TOO_MANY_REQUESTS;
        self.update_at(route, status, headers, body, Instant::now());
    }

    fn update_at(
        &self,
        route: &str,
        status: StatusCode,
        headers: &HeaderMap,
        body: &Value,
        now: Instant,
    ) {
        let text = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
        let seconds = |value: Option<f64>| {
            value
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                .map(Duration::from_secs_f64)
        };
        let header_seconds = |name: &str| seconds(text(name).and_then(|value| value.parse().ok()));
        let reset = header_seconds("x-ratelimit-reset-after");
        let remaining = text("x-ratelimit-remaining").and_then(|value| value.parse().ok());
        let mut inner = self.lock();
        let key = match text("x-ratelimit-bucket") {
            Some(name) => format!("{name}{}", major(route)),
            // A route Discord named no bucket for counts on its own.
            None => inner
                .buckets_by_route
                .get(route)
                .cloned()
                .unwrap_or_else(|| route.to_owned()),
        };
        if status != StatusCode::TOO_MANY_REQUESTS {
            if let (Some(remaining), Some(reset)) = (remaining, reset) {
                inner.buckets.insert(
                    key.clone(),
                    Bucket {
                        remaining,
                        reset: now + reset,
                    },
                );
            }
            if text("x-ratelimit-bucket").is_some() {
                inner.buckets_by_route.insert(route.to_owned(), key);
            }
            return;
        }
        // A 429 says how long to wait in its body, more precisely, or its
        // headers; without either, a second.
        let wait = seconds(body["retry_after"].as_f64())
            .or_else(|| header_seconds("retry-after"))
            .into_iter()
            .chain(reset)
            .max()
            .unwrap_or(Duration::from_secs(1));
        let global = body["global"] == true
            || text("x-ratelimit-global") == Some("true")
            || text("x-ratelimit-scope") == Some("global");
        if global {
            inner.global_until = Some(now + wait);
            return;
        }
        inner.buckets.insert(
            key.clone(),
            Bucket {
                remaining: 0,
                reset: now + wait,
            },
        );
        inner.buckets_by_route.insert(route.to_owned(), key);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The start of a route that Discord counts limits by on their own: its
/// channel, server, or webhook and token. Empty for other routes.
fn major(route: &str) -> &str {
    let path = route.split_once(' ').map_or(route, |(_, path)| path);
    let segments = match path.split('/').find(|segment| !segment.is_empty()) {
        Some("channels" | "guilds") => 2,
        Some("webhooks") => 3,
        _ => return "",
    };
    // The path starts with a slash, so the one after the major segments is
    // that many further on.
    let end = path
        .match_indices('/')
        .nth(segments)
        .map_or(path.len(), |(index, _)| index);
    &path[..end]
}

/// A request's route as Discord counts its limits: the method and path, with
/// the ids in it replaced, except the channel, server or webhook it's about,
/// whose limits are its own. Reactions' emoji are replaced too.
pub fn route(method: &Method, path: &str) -> String {
    let path = path.split_once("/api/v9").map_or(path, |(_, rest)| rest);
    let mut out = format!("{method} ");
    let mut previous = "";
    for (index, segment) in path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .enumerate()
    {
        let major = index == 1 && matches!(previous, "channels" | "guilds" | "webhooks");
        let replaced = if major {
            segment
        } else if previous == "reactions" {
            ":emoji"
        } else if segment.bytes().all(|byte| byte.is_ascii_digit()) {
            ":id"
        } else {
            segment
        };
        out.push('/');
        out.push_str(replaced);
        previous = segment;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;
    use serde_json::json;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn names_routes_by_their_major_id() {
        assert_eq!(
            route(&Method::GET, "/api/v9/channels/12/messages/34"),
            "GET /channels/12/messages/:id"
        );
        assert_eq!(
            route(
                &Method::PUT,
                "/channels/12/messages/34/reactions/%F0%9F%91%8D/@me"
            ),
            "PUT /channels/12/messages/:id/reactions/:emoji/@me"
        );
        assert_eq!(
            route(&Method::GET, "/users/56/profile"),
            "GET /users/:id/profile"
        );
    }

    #[test]
    fn waits_for_an_empty_bucket_and_a_global_limit() {
        let limits = Limits::default();
        let now = Instant::now();
        let route = "POST /channels/1/messages";
        let ok = StatusCode::OK;
        let limited = StatusCode::TOO_MANY_REQUESTS;
        assert_eq!(
            limits.reserve(route, now),
            Duration::ZERO,
            "unknown routes go"
        );
        limits.update_at(
            route,
            ok,
            &headers(&[
                ("x-ratelimit-bucket", "abc"),
                ("x-ratelimit-remaining", "1"),
                ("x-ratelimit-reset-after", "2.5"),
            ]),
            &Value::Null,
            now,
        );
        assert_eq!(limits.reserve(route, now), Duration::ZERO, "one left");
        assert_eq!(limits.reserve(route, now), Duration::from_millis(2500));
        assert_eq!(
            limits.reserve(route, now + Duration::from_secs(3)),
            Duration::ZERO,
            "refilled"
        );

        // A 429 for the bucket empties it until Discord says.
        limits.update_at(
            route,
            limited,
            &headers(&[
                ("x-ratelimit-bucket", "abc"),
                ("x-ratelimit-remaining", "3"),
                ("retry-after", "4"),
            ]),
            &Value::Null,
            now,
        );
        assert_eq!(limits.reserve(route, now), Duration::from_secs(4));

        // A global one holds every route.
        limits.update_at(
            "GET /users/@me",
            limited,
            &headers(&[("x-ratelimit-global", "true"), ("retry-after", "7")]),
            &Value::Null,
            now,
        );
        assert_eq!(limits.reserve("GET /gateway", now), Duration::from_secs(7));
        assert_eq!(
            limits.reserve("GET /gateway", now + Duration::from_secs(8)),
            Duration::ZERO
        );
    }

    #[test]
    fn channels_sharing_a_bucket_count_apart() {
        let limits = Limits::default();
        let now = Instant::now();
        let first = "POST /channels/1/messages";
        let second = "POST /channels/2/messages";
        let answer = |route, remaining| {
            limits.update_at(
                route,
                StatusCode::OK,
                &headers(&[
                    ("x-ratelimit-bucket", "abc"),
                    ("x-ratelimit-remaining", remaining),
                    ("x-ratelimit-reset-after", "5"),
                ]),
                &Value::Null,
                now,
            );
        };
        answer(first, "0");
        answer(second, "4");
        assert_eq!(
            limits.reserve(first, now),
            Duration::from_secs(5),
            "the second channel's answer leaves the first one's count"
        );
        for _ in 0..4 {
            assert_eq!(limits.reserve(second, now), Duration::ZERO);
        }
        assert_eq!(limits.reserve(second, now), Duration::from_secs(5));

        // A 429 in one channel doesn't hold the other.
        let limits = Limits::default();
        answer(second, "4");
        limits.update_at(
            first,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[("x-ratelimit-bucket", "abc"), ("retry-after", "3")]),
            &Value::Null,
            now,
        );
        assert_eq!(limits.reserve(first, now), Duration::from_secs(3));
        assert_eq!(limits.reserve(second, now), Duration::ZERO);
    }

    #[test]
    fn a_429_without_a_bucket_still_waits() {
        let limits = Limits::default();
        let now = Instant::now();
        let route = "PUT /channels/1/messages/:id/reactions/:emoji/@me";
        let limited = StatusCode::TOO_MANY_REQUESTS;
        limits.update_at(
            route,
            limited,
            &HeaderMap::new(),
            &json!({"retry_after": 0.75, "global": false}),
            now,
        );
        assert_eq!(limits.reserve(route, now), Duration::from_millis(750));
        assert_eq!(
            limits.reserve("PUT /channels/2/messages/:id/reactions/:emoji/@me", now),
            Duration::ZERO,
            "another channel goes"
        );
        assert_eq!(
            limits.reserve(route, now + Duration::from_secs(1)),
            Duration::ZERO
        );

        // Only the header.
        limits.update_at(
            route,
            limited,
            &headers(&[("retry-after", "2")]),
            &Value::Null,
            now,
        );
        assert_eq!(limits.reserve(route, now), Duration::from_secs(2));

        // Nothing at all: a second.
        let other = "GET /users/:id/profile";
        limits.update_at(other, limited, &HeaderMap::new(), &Value::Null, now);
        assert_eq!(limits.reserve(other, now), Duration::from_secs(1));

        // Global in the body alone holds every route.
        limits.update_at(
            other,
            limited,
            &HeaderMap::new(),
            &json!({"retry_after": 6.0, "global": true}),
            now,
        );
        assert_eq!(limits.reserve("GET /gateway", now), Duration::from_secs(6));
    }

    #[test]
    fn finds_the_major_part_of_a_route() {
        assert_eq!(major("POST /channels/1/messages"), "/channels/1");
        assert_eq!(major("GET /guilds/2"), "/guilds/2");
        assert_eq!(
            major("POST /webhooks/3/token/messages"),
            "/webhooks/3/token"
        );
        assert_eq!(major("GET /users/@me"), "");
    }
}
