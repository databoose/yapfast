//! Acknowledging reads on Discord: reads within a second go as one
//! acknowledgement per channel.

use std::{collections::HashMap, pin::Pin, sync::Arc};

use tokio::time::{Duration, Instant, Sleep};

use crate::api::Api;

/// Reads within this long go to Discord as one acknowledgement per channel.
const ACK_DELAY: Duration = Duration::from_secs(1);

pub(super) struct ReadAcks {
    api: Arc<Api>,
    /// The newest read message per channel, waiting to be acknowledged.
    waiting: HashMap<String, String>,
    at: Pin<Box<Sleep>>,
}

impl ReadAcks {
    pub(super) fn new(api: Arc<Api>) -> Self {
        Self {
            api,
            waiting: HashMap::new(),
            at: Box::pin(tokio::time::sleep(ACK_DELAY)),
        }
    }

    /// Acknowledges `message` in `channel` with the next batch.
    pub(super) fn read(&mut self, channel: String, message: String) {
        if self.waiting.is_empty() {
            self.at.as_mut().reset(Instant::now() + ACK_DELAY);
        }
        self.waiting.insert(channel, message);
    }

    pub(super) fn waiting(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// Sends the batch once it's due.
    pub(super) async fn send_when_due(&mut self) {
        self.at.as_mut().await;
        for (channel, message) in self.waiting.drain() {
            let api = self.api.clone();
            tokio::spawn(async move {
                if let Err(error) = api.ack(&channel, &message).await {
                    tracing::warn!(%error, "could not mark a Discord channel read");
                }
            });
        }
    }
}
