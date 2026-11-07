use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::box_wallpaper_commands::{WallpaperLog, WallpaperPlan};

pub const UNRESOLVED_PLAN_RETRY_MS: u64 = 60_000;

pub type WallpaperPlanResolver =
    Arc<dyn Fn() -> Option<WallpaperPlan> + Send + Sync + 'static>;
pub type WallpaperPainter = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Clone)]
pub struct WallpaperSchedulerDependencies {
    pub resolve_plan: WallpaperPlanResolver,
    pub paint: WallpaperPainter,
    pub log: WallpaperLog,
}

enum WallpaperSignal {
    Sync,
    Stop,
}

struct WallpaperSchedulerInner {
    deps: WallpaperSchedulerDependencies,
    generation: AtomicU64,
    stopped: AtomicBool,
    sender: Mutex<Option<mpsc::Sender<WallpaperSignal>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct WallpaperToneScheduler {
    inner: Arc<WallpaperSchedulerInner>,
}

impl WallpaperToneScheduler {
    pub fn new(deps: WallpaperSchedulerDependencies) -> Self {
        Self {
            inner: Arc::new(WallpaperSchedulerInner {
                deps,
                generation: AtomicU64::new(0),
                stopped: AtomicBool::new(false),
                sender: Mutex::new(None),
                worker: Mutex::new(None),
            }),
        }
    }

    pub fn start(&self) {
        let mut sender_slot = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if sender_slot.is_some() || self.inner.stopped.load(Ordering::Acquire) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        *sender_slot = Some(sender);
        drop(sender_slot);

        let inner = Arc::clone(&self.inner);
        let worker = thread::Builder::new()
            .name("mahayana-wallpaper-tone".into())
            .spawn(move || run_scheduler(inner, receiver))
            .expect("wallpaper scheduler thread should start");
        *self
            .inner
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(worker);
        self.begin_sync();
    }

    pub fn begin_sync(&self) {
        if self.inner.stopped.load(Ordering::Acquire) {
            return;
        }
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
        if let Some(sender) = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            let _ = sender.send(WallpaperSignal::Sync);
        }
    }

    pub fn stop(&self) {
        if self.inner.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(sender) = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = sender.send(WallpaperSignal::Stop);
        }
        if let Some(worker) = self
            .inner
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = worker.join();
        }
    }

    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Acquire)
    }
}

pub fn remaining_wallpaper_delay_ms(
    plan_ms: u64,
    paint_elapsed: Duration,
) -> u64 {
    plan_ms.saturating_sub(
        paint_elapsed
            .as_millis()
            .min(u64::MAX as u128) as u64,
    )
}

fn run_scheduler(
    inner: Arc<WallpaperSchedulerInner>,
    receiver: mpsc::Receiver<WallpaperSignal>,
) {
    let mut should_sync = false;
    loop {
        if inner.stopped.load(Ordering::Acquire) {
            return;
        }
        if !should_sync {
            match receiver.recv() {
                Ok(WallpaperSignal::Sync) => {}
                Ok(WallpaperSignal::Stop) | Err(_) => return,
            }
        }
        should_sync = false;

        let generation = inner.generation.load(Ordering::Acquire);
        let plan = (inner.deps.resolve_plan)();
        if is_stale(&inner, generation) {
            should_sync = true;
            continue;
        }

        let Some(plan) = plan else {
            should_sync = wait_for_next(
                &inner,
                &receiver,
                Duration::from_millis(UNRESOLVED_PLAN_RETRY_MS),
            );
            if !should_sync {
                return;
            }
            continue;
        };

        let started = Instant::now();
        (inner.deps.paint)();
        if is_stale(&inner, generation) {
            should_sync = true;
            continue;
        }

        let remaining_ms =
            remaining_wallpaper_delay_ms(plan.ms_until_next_boundary, started.elapsed());
        (inner.deps.log)(&format!(
            "wallpaper: painted tone {}; next boundary in {}s",
            plan.tone,
            ((remaining_ms as f64) / 1_000.0).round() as u64,
        ));

        should_sync = wait_for_next(
            &inner,
            &receiver,
            Duration::from_millis(remaining_ms),
        );
        if !should_sync {
            return;
        }
    }
}

fn is_stale(inner: &WallpaperSchedulerInner, generation: u64) -> bool {
    inner.stopped.load(Ordering::Acquire)
        || generation != inner.generation.load(Ordering::Acquire)
}

fn wait_for_next(
    inner: &WallpaperSchedulerInner,
    receiver: &mpsc::Receiver<WallpaperSignal>,
    delay: Duration,
) -> bool {
    if inner.stopped.load(Ordering::Acquire) {
        return false;
    }
    match receiver.recv_timeout(delay) {
        Ok(WallpaperSignal::Sync) => true,
        Ok(WallpaperSignal::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => false,
        Err(mpsc::RecvTimeoutError::Timeout) => true,
    }
}
