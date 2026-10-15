//! How a call's connection is doing, as Discord's client shows it in its
//! voice panel: the voice gateway's ping, the share of this account's
//! packets lost on the way to Discord's server, and whether the call is
//! end-to-end encrypted, with its voice privacy code. The call's thread
//! writes it as heartbeats, RTCP and DAVE's messages come; Talk reads it
//! only while it shows it.

use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
    time::Duration,
};

use crate::voice_connection::lock;

/// How often the voice gateway is sent a heartbeat, and so pinged: as
/// Discord's web client caps the interval the server asks for.
pub const PING_EVERY: Duration = Duration::from_secs(5);
/// The pings kept: five minutes of them, about what Discord's graph shows.
pub const PINGS: usize = 60;

/// What the call's thread has measured, behind a lock it holds only to
/// write a sample or copy them out.
#[derive(Debug, Default)]
pub struct CallStats(Mutex<Stats>);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    /// The voice gateway's heartbeat round trips, in milliseconds, oldest
    /// first, at most [`PINGS`].
    pub pings: VecDeque<u16>,
    /// The percentage of this account's packets lost between the last two
    /// receiver reports Discord sent about them, once one has come.
    pub loss: Option<f32>,
    /// Frames leave end-to-end encrypted with DAVE.
    pub end_to_end: bool,
    /// The DAVE group's voice privacy code, 30 digits, while end-to-end
    /// encrypted.
    pub privacy_code: Option<String>,
}

impl CallStats {
    /// A copy of everything measured.
    pub fn read(&self) -> Stats {
        lock(&self.0).clone()
    }

    /// The latest ping and loss, and whether the call is end-to-end
    /// encrypted, without copying the rest.
    pub fn summary(&self) -> (Option<u16>, Option<f32>, bool) {
        let stats = lock(&self.0);
        (stats.pings.back().copied(), stats.loss, stats.end_to_end)
    }

    pub(crate) fn ping(&self, round_trip: Duration) {
        let mut stats = lock(&self.0);
        if stats.pings.capacity() < PINGS {
            stats.pings.reserve_exact(PINGS);
        }
        if stats.pings.len() == PINGS {
            stats.pings.pop_front();
        }
        stats
            .pings
            .push_back(round_trip.as_millis().min(u128::from(u16::MAX)) as u16);
    }

    pub(crate) fn encryption(&self, end_to_end: bool, code: Option<&str>) {
        let mut stats = lock(&self.0);
        stats.end_to_end = end_to_end;
        let code = code.filter(|_| end_to_end);
        if stats.privacy_code.as_deref() != code {
            stats.privacy_code = code.map(str::to_owned);
        }
    }

    fn loss(&self, loss: f32) {
        lock(&self.0).loss = Some(loss);
    }
}

/// What the last receiver report said about each of this account's SSRCs:
/// packets lost so far, and the highest sequence number received.
#[derive(Default)]
pub(crate) struct Reported(HashMap<u32, (u32, u32)>);

impl Reported {
    /// Takes the report blocks about `ours` from a decrypted compound RTCP
    /// packet, and records the share of packets lost since the last report
    /// on each, together, as Discord's client sums its outbound streams.
    pub fn learn(&mut self, stats: &CallStats, rtcp: &[u8], ours: &[u32]) {
        let (mut lost, mut expected) = (0i64, 0i64);
        let mut compared = false;
        for (ssrc, lost_now, highest) in crate::rtp::report_blocks(rtcp) {
            if !ours.contains(&ssrc) {
                continue;
            }
            if let Some((lost_before, highest_before)) = self.0.insert(ssrc, (lost_now, highest)) {
                compared = true;
                lost += i64::from(lost_now) - i64::from(lost_before);
                expected += i64::from(highest.wrapping_sub(highest_before));
            }
        }
        if compared {
            // Nothing sent since: nothing lost, as Discord's client says.
            let share = if expected == 0 {
                0.0
            } else {
                (lost as f32 / expected as f32).clamp(0.0, 1.0) * 100.0
            };
            stats.loss(share);
        }
    }

    /// A new connection numbers its packets afresh.
    pub fn reset(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtp::receiver_report;

    #[test]
    fn keeps_the_last_pings() {
        let stats = CallStats::default();
        for ping in 0..PINGS as u64 + 5 {
            stats.ping(Duration::from_millis(ping));
        }
        let read = stats.read();
        assert_eq!(read.pings.len(), PINGS);
        assert_eq!(read.pings.front(), Some(&5));
        assert_eq!(stats.summary().0, Some(PINGS as u16 + 4));
    }

    #[test]
    fn loss_is_the_share_lost_between_reports() {
        let (stats, mut reported) = (CallStats::default(), Reported::default());
        let report = |lost, highest| receiver_report(1, 7, (highest, lost, 0), (0, 0));
        reported.learn(&stats, &report(2, 100), &[7]);
        assert_eq!(stats.read().loss, None, "nothing to compare yet");
        reported.learn(&stats, &report(7, 200), &[7]);
        assert_eq!(stats.read().loss, Some(5.0));
        // Another's SSRC is no concern of this account's.
        reported.learn(&stats, &receiver_report(1, 8, (900, 900, 0), (0, 0)), &[7]);
        assert_eq!(stats.read().loss, Some(5.0));
        reported.learn(&stats, &report(7, 200), &[7]);
        assert_eq!(stats.read().loss, Some(0.0), "nothing sent since");
    }

    #[test]
    fn keeps_the_privacy_code_only_while_end_to_end() {
        let stats = CallStats::default();
        stats.encryption(true, Some("123"));
        assert_eq!(stats.read().privacy_code.as_deref(), Some("123"));
        stats.encryption(false, Some("123"));
        assert_eq!(stats.read().privacy_code, None);
    }
}
