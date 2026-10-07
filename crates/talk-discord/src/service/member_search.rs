//! Member searches for mentions: each waits for the chunk of members
//! Discord sends back with its nonce.

use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::oneshot;

use super::tracker::Tracker;
use crate::gateway::{Dispatch, Gateway};

/// The most members one search asks for.
const FOUND: u64 = 10;

/// Member searches waiting for their answer, by nonce.
#[derive(Default)]
pub(super) struct MemberSearches {
    waiting: HashMap<String, oneshot::Sender<Vec<String>>>,
    nonce: u64,
}

impl MemberSearches {
    /// Asks Discord for the members of `guild` whose names start with `query`.
    pub(super) async fn start(
        &mut self,
        gateway: &mut Gateway,
        guild: &str,
        query: &str,
        reply: oneshot::Sender<Vec<String>>,
    ) -> Result<()> {
        self.nonce += 1;
        let nonce = format!("talk-search-{}", self.nonce);
        // Unanswered searches are dropped, so their callers stop waiting.
        self.waiting.retain(|_, reply| !reply.is_closed());
        self.waiting.insert(nonce.clone(), reply);
        gateway.search_members(guild, query, FOUND, &nonce).await
    }

    /// Answers the search a chunk of members is for, if any.
    pub(super) fn answer(&mut self, tracker: &mut Tracker, event: &Dispatch) {
        let Some(reply) = event.data["nonce"]
            .as_str()
            .and_then(|nonce| self.waiting.remove(nonce))
        else {
            return;
        };
        // Learn the members before answering, so their names are known.
        tracker.apply(&event.event, &event.data);
        let found = event.data["members"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|member| member["user"]["id"].as_str())
            .map(str::to_owned)
            .collect();
        let _ = reply.send(found);
    }
}
