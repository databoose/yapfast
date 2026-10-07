//! Discord's HTTP rate limits, which its API asks every client to keep. Each
//! route belongs to a bucket, named in `X-RateLimit-Bucket`, that allows
//! `X-RateLimit-Remaining` more requests until `X-RateLimit-Reset-After`
//! seconds pass, and a 429 marked global pauses every route. Talk waits
//! rather than sending a request Discord would refuse: refusals count against
//! the account, and too many of them get its address blocked for a while.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use reqwest::{Method, StatusCode, header::HeaderMap};

/// The longest Talk waits on a limit before giving up on the request.
pub const MAX_WAIT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct Limits {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The bucket each route belongs to, as Discord last said.
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
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(until) = inner.global_until {
            if until > now {
                return until - now;
            }
            inner.global_until = None;
        }
        let Some(name) = inner.buckets_by_route.get(route).cloned() else {
            return Duration::ZERO;
        };
        let Some(bucket) = inner.buckets.get_mut(&name) else {
            return Duration::ZERO;
        };
        if bucket.reset <= now {
            // The bucket refilled; Discord's next answer says by how much.
            inner.buckets.remove(&name);
            return Duration::ZERO;
        }
        if bucket.remaining == 0 {
            return bucket.reset - now;
        }
        bucket.remaining -= 1;
        Duration::ZERO
    }

    /// Learns the limits from an answer to a request on `route`.
    pub fn update(&self, route: &str, status: StatusCode, headers: &HeaderMap) {
        self.update_at(route, status, headers, Instant::now());
    }

    fn update_at(&self, route: &str, status: StatusCode, headers: &HeaderMap, now: Instant) {
        let text = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
        let seconds = |name: &str| {
            text(name)
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                .map(Duration::from_secs_f64)
        };
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if status == StatusCode::TOO_MANY_REQUESTS
            && text("x-ratelimit-global").is_some_and(|global| global == "true")
        {
            let wait = seconds("retry-after").unwrap_or(Duration::from_secs(1));
            inner.global_until = Some(now + wait);
            return;
        }
        let Some(name) = text("x-ratelimit-bucket").map(str::to_owned) else {
            return;
        };
        let remaining = text("x-ratelimit-remaining").and_then(|value| value.parse().ok());
        let reset = seconds("x-ratelimit-reset-after").or_else(|| {
            (status == StatusCode::TOO_MANY_REQUESTS)
                .then(|| seconds("retry-after"))
                .flatten()
        });
        inner
            .buckets_by_route
            .insert(route.to_owned(), name.clone());
        if let (Some(remaining), Some(reset)) = (remaining, reset) {
            let remaining = if status == StatusCode::TOO_MANY_REQUESTS {
                0
            } else {
                remaining
            };
            inner.buckets.insert(
                name,
                Bucket {
                    remaining,
                    reset: now + reset,
                },
            );
        }
    }
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
        assert_eq!(
            limits.reserve(route, now),
            Duration::ZERO,
            "unknown routes go"
        );
        limits.update_at(
            route,
            StatusCode::OK,
            &headers(&[
                ("x-ratelimit-bucket", "abc"),
                ("x-ratelimit-remaining", "1"),
                ("x-ratelimit-reset-after", "2.5"),
            ]),
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
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[
                ("x-ratelimit-bucket", "abc"),
                ("x-ratelimit-remaining", "3"),
                ("retry-after", "4"),
            ]),
            now,
        );
        assert_eq!(limits.reserve(route, now), Duration::from_secs(4));

        // A global one holds every route.
        limits.update_at(
            "GET /users/@me",
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[("x-ratelimit-global", "true"), ("retry-after", "7")]),
            now,
        );
        assert_eq!(limits.reserve("GET /gateway", now), Duration::from_secs(7));
        assert_eq!(
            limits.reserve("GET /gateway", now + Duration::from_secs(8)),
            Duration::ZERO
        );
    }
}
