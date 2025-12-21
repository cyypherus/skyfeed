use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) struct UpdatesCounter {
    count: Arc<AtomicU64>,
    last_log: Arc<tokio::sync::Mutex<Instant>>,
}

impl UpdatesCounter {
    pub(crate) fn new() -> Self {
        UpdatesCounter {
            count: Arc::new(AtomicU64::new(0)),
            last_log: Arc::new(tokio::sync::Mutex::new(Instant::now())),
        }
    }

    pub(crate) async fn increment_and_maybe_log(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
        let mut last_log = self.last_log.lock().await;
        let elapsed = last_log.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let count = self.count.swap(0, Ordering::Relaxed);
            let ups = count as f64 / elapsed.as_secs_f64();
            log::trace!("updates/sec: {:.2}", ups);
            *last_log = Instant::now();
        }
    }
}
