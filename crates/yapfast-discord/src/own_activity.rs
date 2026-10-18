//! One activity for the Gateway on every platform. Rich Presence takes
//! precedence over automatic detection, while custom status stays independent.

use crate::{api::Api, service::Snapshot};
use serde_json::Value;
use std::sync::{Arc, OnceLock, RwLock};
use tokio::sync::watch;

type Activity = Option<Arc<Value>>;
type Current = Option<(String, Option<u64>)>;

fn current_slot() -> &'static RwLock<Current> {
    static CURRENT: OnceLock<RwLock<Current>> = OnceLock::new();
    CURRENT.get_or_init(|| RwLock::new(None))
}

pub(crate) fn current() -> Option<(String, Option<u64>)> {
    current_slot()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn publish(activity: Option<&Value>) {
    let current = activity.and_then(|activity| {
        let name = activity.get("name")?.as_str()?.trim();
        (!name.is_empty()).then(|| {
            (
                name.to_owned(),
                activity
                    .pointer("/timestamps/start")
                    .and_then(Value::as_u64),
            )
        })
    });
    *current_slot()
        .write()
        .unwrap_or_else(|error| error.into_inner()) = current;
}

pub(crate) fn start(
    api: Arc<Api>,
    snapshots: watch::Receiver<Arc<Snapshot>>,
) -> watch::Receiver<Activity> {
    #[cfg(not(target_os = "linux"))]
    let _ = snapshots;
    let (shown, receiver) = watch::channel(None);
    tokio::spawn(async move {
        let mut settings = crate::game_detection::preferences::subscribe();
        loop {
            if settings.borrow_and_update().enabled {
                let detected = crate::game_detection::start(api.clone(), settings.clone());
                #[cfg(target_os = "linux")]
                let rpc = crate::rpc::start(api.clone(), snapshots.clone());
                #[cfg(not(target_os = "linux"))]
                let rpc = watch::channel(None).1;
                tokio::select! {
                    () = combine(rpc, detected, shown.clone()) => return,
                    _ = async {
                        loop {
                            if settings.changed().await.is_err() || !settings.borrow().enabled { break; }
                        }
                    } => {}
                }
                shown.send_replace(None);
                publish(None);
            } else {
                tokio::select! {
                    () = shown.closed() => return,
                    result = settings.changed() => if result.is_err() { return; },
                }
            }
        }
    });
    receiver
}

async fn combine(
    mut rpc: watch::Receiver<Activity>,
    mut detected: watch::Receiver<Activity>,
    shown: watch::Sender<Activity>,
) {
    let mut rpc_open = true;
    let mut detection_open = true;
    loop {
        let next = rpc_open
            .then(|| rpc.borrow().clone())
            .flatten()
            .or_else(|| detection_open.then(|| detected.borrow().clone()).flatten());
        publish(next.as_deref());
        shown.send_if_modified(|old| {
            if *old == next {
                false
            } else {
                *old = next;
                true
            }
        });
        tokio::select! {
            () = shown.closed() => { publish(None); return; },
            result = rpc.changed(), if rpc_open => rpc_open = result.is_ok(),
            result = detected.changed(), if detection_open => detection_open = result.is_ok(),
        }
    }
}

pub(crate) fn merge(presence: &mut Value, activity: Option<&Value>) {
    if let Some(activities) = presence["activities"].as_array_mut() {
        activities.retain(|value| value["type"] == 4);
        activities.extend(activity.cloned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn rpc_overrides_detection_and_disconnect_restores_it() {
        let (rpc, rpc_rx) = watch::channel(None);
        let (detected, detected_rx) = watch::channel(None);
        let (shown, mut receiver) = watch::channel(None);
        let worker = tokio::spawn(combine(rpc_rx, detected_rx, shown));
        let game = Arc::new(json!({"type":0,"name":"Game"}));
        detected.send_replace(Some(game.clone()));
        receiver.changed().await.unwrap();
        assert_eq!(receiver.borrow_and_update().as_ref(), Some(&game));
        let rich = Arc::new(json!({"type":0,"name":"Game","details":"In match"}));
        rpc.send_replace(Some(rich.clone()));
        receiver.changed().await.unwrap();
        assert_eq!(receiver.borrow_and_update().as_ref(), Some(&rich));
        // A source that dies without clearing must not leave a stale activity.
        drop(rpc);
        receiver.changed().await.unwrap();
        assert_eq!(receiver.borrow_and_update().as_ref(), Some(&game));
        detected.send_replace(None);
        receiver.changed().await.unwrap();
        assert!(receiver.borrow_and_update().is_none());
        drop(receiver);
        worker.await.unwrap();
        assert!(detected.is_closed());
    }
}
