use tracing_subscriber::{filter::LevelFilter, filter::Targets, prelude::*};

pub fn init() {
    // Dependency debug logs can contain tokens or message content. Keep them off.
    let filter = Targets::new()
        .with_target("talk", LevelFilter::INFO)
        .with_target("talk_core", LevelFilter::INFO);
    // Several tests in one process may call this; the first subscriber stays.
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter))
        .try_init();
}
