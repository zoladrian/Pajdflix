//! Diagnose blocked UI work in release builds as well as developer builds.
use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

static PHASE: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Phase {
    WindowShow = 1,
    ModelUpdate = 2,
    MpvRendering = 3,
}

fn phase_name(value: u8) -> &'static str {
    match value {
        1 => "window_show",
        2 => "model_update",
        3 => "mpv_render_callback",
        _ => "event_loop_or_slint_rendering",
    }
}

pub struct Work {
    phase: u8,
    previous: u8,
    started: Instant,
}
impl Work {
    pub fn begin(phase: Phase) -> Self {
        Self {
            phase: phase as u8,
            previous: PHASE.swap(phase as u8, Ordering::Relaxed),
            started: Instant::now(),
        }
    }
}
impl Drop for Work {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        PHASE.store(self.previous, Ordering::Relaxed);
        if elapsed >= Duration::from_millis(250) {
            tracing::warn!(
                phase = phase_name(self.phase),
                elapsed_ms = elapsed.as_millis(),
                "ui_work_slow"
            );
        }
    }
}

pub struct Watchdog {
    heartbeat: slint::Timer,
    monitor: tokio::task::JoinHandle<()>,
}
impl Watchdog {
    pub fn stop(&self) {
        self.heartbeat.stop();
        self.monitor.abort();
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start() -> Watchdog {
    let epoch = Instant::now();
    let last_tick = Arc::new(AtomicU64::new(0));
    let heartbeat_tick = last_tick.clone();
    let heartbeat = slint::Timer::default();
    heartbeat.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            let now = epoch.elapsed().as_millis() as u64;
            let previous = heartbeat_tick.swap(now, Ordering::Relaxed);
            if now.saturating_sub(previous) >= 2000 {
                tracing::warn!(elapsed_ms = now - previous, "ui_event_loop_recovered");
            }
        },
    );
    let monitor = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let mut reported_tick = None;
        loop {
            interval.tick().await;
            let tick = last_tick.load(Ordering::Relaxed);
            let delay = (epoch.elapsed().as_millis() as u64).saturating_sub(tick);
            if delay >= 2000 && reported_tick != Some(tick) {
                reported_tick = Some(tick);
                tracing::warn!(
                    phase = phase_name(PHASE.load(Ordering::Relaxed)),
                    elapsed_ms = delay,
                    "ui_event_loop_stalled"
                );
            }
        }
    });
    Watchdog { heartbeat, monitor }
}
