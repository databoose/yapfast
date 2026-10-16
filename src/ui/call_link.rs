//! The call's connection in detail: the ping as the signal icon's tooltip,
//! and the popout's graph, numbers and voice privacy code. The call measures
//! them all along; they're copied out of it only while one of them shows.

use std::fmt::Write;

use yapfast_core::voice::LinkStats;

use super::*;

/// The graph's box, as `CallLinkCard`'s path takes it, and the pings it
/// spans: five minutes of them, as the call keeps.
const GRAPH: (f32, f32) = (300.0, 100.0);
const SPAN: usize = 60;

pub(super) fn bind(window: &YapfastWindow, adapter: &Rc<RefCell<Adapter>>) {
    let state = Rc::downgrade(adapter);
    let weak = window.as_weak();
    window.global::<CallLink>().on_read(move || {
        let (Some(adapter), Some(window)) = (state.upgrade(), weak.upgrade()) else {
            return;
        };
        let Ok(adapter) = adapter.try_borrow() else {
            return;
        };
        let voice = &adapter.app.state.voice;
        let stats = voice
            .link
            .as_ref()
            .map(yapfast_core::voice::Link::read)
            .unwrap_or_default();
        show(
            &window.global::<CallLink>(),
            &stats,
            voice.participants.len() > 1,
        );
    });
}

/// Shows `stats`, with the privacy code only while `others` are in the call
/// to compare it with, as Discord's client does.
fn show(link: &CallLink<'_>, stats: &LinkStats, others: bool) {
    let milliseconds = |ping: u32| format!("{ping} ms");
    let last = stats.pings.last().map(|&ping| milliseconds(ping.into()));
    link.set_ping(last.unwrap_or_default().into());
    let average = average(&stats.pings).map(milliseconds);
    link.set_average(average.unwrap_or_default().into());
    let loss = stats.loss.map(|loss| format!("{loss:.1}%"));
    link.set_loss(loss.unwrap_or_default().into());
    let (graph, top) = graph(&stats.pings);
    link.set_graph(graph.into());
    link.set_graph_top(top as i32);
    let code = stats.privacy_code.as_deref().filter(|_| others);
    let groups = code.map(groups).unwrap_or_default();
    let shown = link.get_code();
    if !shown.iter().eq(groups.iter().cloned()) {
        link.set_code(slint::ModelRc::new(slint::VecModel::from(groups)));
    }
}

fn average(pings: &[u16]) -> Option<u32> {
    let sum: u32 = pings.iter().copied().map(u32::from).sum();
    (!pings.is_empty()).then(|| sum / pings.len() as u32)
}

/// The pings as steps, the newest at the right edge, and the ping at the
/// top: the smallest 1, 2 or 5 times a power of ten above them all.
fn graph(pings: &[u16]) -> (String, u32) {
    let highest = pings.iter().copied().max().map_or(0, u32::from);
    let top = [1, 2, 5]
        .into_iter()
        .flat_map(|step| (1..6).map(move |power| step * 10u32.pow(power)))
        .filter(|&top| top >= highest)
        .min()
        .unwrap_or(highest);
    let step = GRAPH.0 / SPAN as f32;
    let y = |ping: u16| GRAPH.1 - f32::from(ping) / top as f32 * GRAPH.1;
    let first = SPAN.saturating_sub(pings.len());
    let mut path = String::new();
    // Each ping holds for the step before it, so even one shows.
    for (index, &ping) in pings.iter().rev().take(SPAN).rev().enumerate() {
        let x = (first + index + 1) as f32 * step;
        let _ = if path.is_empty() {
            write!(path, "M {:.1} {:.1} H {x:.1}", x - step, y(ping))
        } else {
            write!(path, " V {:.1} H {x:.1}", y(ping))
        };
    }
    (path, top)
}

/// A privacy code in the groups of five digits it's read in.
fn groups(code: &str) -> Vec<slint::SharedString> {
    code.as_bytes()
        .chunks(5)
        .map(|group| String::from_utf8_lossy(group).as_ref().into())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphs_the_pings_up_to_a_round_top() {
        assert_eq!(graph(&[]), (String::new(), 10));
        let (path, top) = graph(&[40, 80]);
        assert_eq!(top, 100);
        assert_eq!(path, "M 290.0 60.0 H 295.0 V 20.0 H 300.0");
        assert_eq!(graph(&[260]).1, 500);
        assert_eq!(graph(&[1500]).1, 2000);
    }

    #[test]
    fn reads_the_code_in_groups_of_five() {
        let groups = groups("123456789012345678901234567890");
        assert_eq!(groups.len(), 6);
        assert_eq!(groups[1], "67890");
        assert_eq!(average(&[10, 20, 31]), Some(20));
        assert_eq!(average(&[]), None);
    }
}
