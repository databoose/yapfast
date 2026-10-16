//! Publishing snapshots of the account: the first change after a quiet
//! spell goes out at once, and a burst of them together.

use std::{pin::Pin, sync::Arc};

use tokio::{
    sync::watch,
    time::{Duration, Instant, Sleep},
};

use super::{Snapshot, member_lists::MemberLists, tracker::Tracker};

/// Snapshots go out at most this often.
const PUBLISH_GAP: Duration = Duration::from_millis(100);

pub(super) struct Publisher {
    snapshots: watch::Sender<Arc<Snapshot>>,
    /// The account changed since the last snapshot; one goes out when `at`
    /// comes, [`PUBLISH_GAP`] after the last.
    stale: bool,
    published: Instant,
    at: Pin<Box<Sleep>>,
}

impl Publisher {
    pub(super) fn new(snapshots: watch::Sender<Arc<Snapshot>>) -> Self {
        Self {
            snapshots,
            stale: false,
            published: Instant::now(),
            at: Box::pin(tokio::time::sleep(Duration::ZERO)),
        }
    }

    /// Notes that the account changed, so a snapshot goes out soon.
    pub(super) fn changed(&mut self) {
        if !self.stale {
            self.stale = true;
            self.at
                .as_mut()
                .reset((self.published + PUBLISH_GAP).max(Instant::now()));
        }
    }

    pub(super) fn stale(&self) -> bool {
        self.stale
    }

    /// Waits until the next snapshot is due.
    pub(super) async fn due(&mut self) {
        self.at.as_mut().await;
    }

    /// Publishes the account as it is now.
    pub(super) fn publish(&mut self, tracker: &mut Tracker, lists: &MemberLists) {
        let mut snapshot = tracker.snapshot();
        snapshot.listed = lists.listed();
        self.snapshots.send_replace(Arc::new(snapshot));
        self.published = Instant::now();
        self.stale = false;
    }
}
