use std::{
    cmp::Ordering,
    collections::VecDeque,
    mem::MaybeUninit,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
    time::{Duration, Instant},
};

use futures::{StreamExt as _, channel::mpsc};
use gpui::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Task, Timer,
    Window, div, px,
};
use gpui_component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, get_current_pid};

const FRAME_HISTORY_CAPACITY: usize = 120;
const FRAME_BURST_GAP: Duration = Duration::from_millis(250);
const FRAME_IDLE_TIMEOUT: Duration = Duration::from_secs(1);
const RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const BYTES_PER_MIB: f64 = 1024.0 * 1024.0;

/// Rolling UI redraw statistics.
///
/// These values describe GPUI view redraw cadence, not GPU presentation time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    pub ui_fps: f64,
    pub average_frame_ms: f64,
    pub p95_frame_ms: f64,
    pub sample_count: usize,
}

/// Resource usage for this application process.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ResourceSample {
    /// Process CPU on sysinfo's logical-core scale. This can exceed 100%.
    pub cpu_percent: Option<f32>,
    /// Resident set size in bytes.
    pub rss_bytes: u64,
    /// Activity Monitor-style physical footprint on macOS.
    pub physical_footprint_bytes: Option<u64>,
}

#[derive(Debug)]
struct FrameHistory {
    frame_times: VecDeque<Duration>,
    last_frame_at: Option<Instant>,
    capacity: usize,
}

impl FrameHistory {
    fn new(capacity: usize) -> Self {
        Self {
            frame_times: VecDeque::with_capacity(capacity),
            last_frame_at: None,
            capacity: capacity.max(1),
        }
    }

    fn reset(&mut self) {
        self.frame_times.clear();
        self.last_frame_at = None;
    }

    fn record_frame(&mut self, now: Instant) {
        if let Some(previous) = self.last_frame_at {
            let frame_time = now.saturating_duration_since(previous);
            if frame_time <= FRAME_BURST_GAP {
                self.push_frame_time(frame_time);
            } else {
                self.frame_times.clear();
            }
        }
        self.last_frame_at = Some(now);
    }

    fn push_frame_time(&mut self, frame_time: Duration) {
        if frame_time.is_zero() {
            return;
        }

        if self.frame_times.len() == self.capacity {
            self.frame_times.pop_front();
        }
        self.frame_times.push_back(frame_time);
    }

    fn stats(&self) -> FrameStats {
        if self.frame_times.is_empty() {
            return FrameStats::default();
        }

        let mut milliseconds: Vec<_> = self
            .frame_times
            .iter()
            .map(|duration| duration.as_secs_f64() * 1_000.0)
            .collect();
        let total_ms: f64 = milliseconds.iter().sum();
        let average_frame_ms = total_ms / milliseconds.len() as f64;
        milliseconds.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
        let p95_index = ((milliseconds.len() as f64 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(milliseconds.len() - 1);

        FrameStats {
            ui_fps: if average_frame_ms > 0.0 {
                1_000.0 / average_frame_ms
            } else {
                0.0
            },
            average_frame_ms,
            p95_frame_ms: milliseconds[p95_index],
            sample_count: milliseconds.len(),
        }
    }

    fn live_stats(&self, now: Instant) -> FrameStats {
        if self
            .last_frame_at
            .is_none_or(|last| now.saturating_duration_since(last) >= FRAME_IDLE_TIMEOUT)
        {
            FrameStats::default()
        } else {
            self.stats()
        }
    }
}

/// Toggleable in-app performance HUD.
///
/// The overlay defaults to hidden. The application view records real GPUI
/// redraws into this entity; the HUD itself never creates an animation loop.
/// CPU and RSS collection runs on the background executor once per second and
/// never blocks the UI thread.
pub struct DebugOverlay {
    visible: bool,
    frame_history: Arc<Mutex<FrameHistory>>,
    resource_sample: Option<ResourceSample>,
    sampler_enabled: Arc<AtomicBool>,
    _resource_sampler: Task<()>,
}

impl DebugOverlay {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let sampler_enabled = Arc::new(AtomicBool::new(false));
        let (sender, mut receiver) = mpsc::unbounded();
        let resource_sampler =
            cx.background_spawn(sample_current_process(sender, Arc::clone(&sampler_enabled)));

        cx.spawn(async move |this, cx| {
            while let Some(sample) = receiver.next().await {
                let Some(this) = this.upgrade() else {
                    break;
                };
                if this
                    .update(cx, |this, cx| {
                        this.resource_sample = Some(sample);
                        if this.visible {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            visible: false,
            frame_history: Arc::new(Mutex::new(FrameHistory::new(FRAME_HISTORY_CAPACITY))),
            resource_sample: None,
            sampler_enabled,
            _resource_sampler: resource_sampler,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.set_visible(!self.visible, cx);
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }

        self.visible = visible;
        lock_frame_history(&self.frame_history).reset();
        self.sampler_enabled.store(visible, AtomicOrdering::Release);
        if visible {
            self.resource_sample = None;
        }
        cx.notify();
    }

    /// Record one redraw of the application view. Hidden overlays keep this
    /// path to a single atomic load.
    pub fn record_ui_frame(&self) {
        if self.sampler_enabled.load(AtomicOrdering::Acquire) {
            lock_frame_history(&self.frame_history).record_frame(Instant::now());
        }
    }

    #[allow(dead_code)]
    pub fn frame_stats(&self) -> FrameStats {
        lock_frame_history(&self.frame_history).stats()
    }

    #[allow(dead_code)]
    pub fn resource_sample(&self) -> Option<ResourceSample> {
        self.resource_sample
    }
}

impl Render for DebugOverlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div();
        }

        let frame_stats = lock_frame_history(&self.frame_history).live_stats(Instant::now());
        let (cpu, rss, footprint) = match self.resource_sample {
            Some(sample) => (
                format_cpu_percent(sample.cpu_percent),
                format_rss_mib(sample.rss_bytes),
                sample
                    .physical_footprint_bytes
                    .map(format_rss_mib)
                    .unwrap_or_else(|| "unavailable".to_owned()),
            ),
            None => (
                "warming up".to_owned(),
                "warming up".to_owned(),
                "warming up".to_owned(),
            ),
        };

        let metric_row = |label: &'static str, value: String| {
            h_flex()
                .w_full()
                .justify_between()
                .gap_4()
                .child(div().text_color(cx.theme().muted_foreground).child(label))
                .child(div().font_semibold().child(value))
        };

        v_flex()
            .absolute()
            .bottom(px(12.0))
            .right(px(12.0))
            .w(px(228.0))
            .gap_1()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover.opacity(0.96))
            .text_color(cx.theme().popover_foreground)
            .font_family(cx.theme().mono_font_family.clone())
            .text_xs()
            .shadow_lg()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .mb_1()
                    .child(div().font_semibold().child("PERFORMANCE"))
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child("event-driven"),
                    ),
            )
            .child(metric_row(
                "UI FPS",
                format_ui_fps(frame_stats.ui_fps, frame_stats.sample_count),
            ))
            .child(metric_row(
                "Frame avg",
                format_frame_ms(frame_stats.average_frame_ms, frame_stats.sample_count),
            ))
            .child(metric_row(
                "Frame p95",
                format_frame_ms(frame_stats.p95_frame_ms, frame_stats.sample_count),
            ))
            .child(metric_row("Process CPU", cpu))
            .child(metric_row("RSS", rss))
            .child(metric_row("Footprint", footprint))
    }
}

async fn sample_current_process(
    sender: mpsc::UnboundedSender<ResourceSample>,
    enabled: Arc<AtomicBool>,
) {
    let Ok(pid) = get_current_pid() else {
        return;
    };
    let refresh_kind = ProcessRefreshKind::nothing().with_cpu().with_memory();
    let mut system = System::new();
    let mut cpu_is_primed = false;

    loop {
        if enabled.load(AtomicOrdering::Acquire) {
            system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, refresh_kind);

            if let Some(process) = system.process(pid) {
                let sample = ResourceSample {
                    cpu_percent: cpu_is_primed.then(|| process.cpu_usage()),
                    rss_bytes: process.memory(),
                    physical_footprint_bytes: current_physical_footprint(),
                };
                cpu_is_primed = true;
                if sender.unbounded_send(sample).is_err() {
                    break;
                }
            }
        } else {
            cpu_is_primed = false;
        }

        Timer::after(RESOURCE_SAMPLE_INTERVAL).await;
    }
}

#[cfg(target_os = "macos")]
fn current_physical_footprint() -> Option<u64> {
    let mut usage = MaybeUninit::<libc::rusage_info_v2>::uninit();
    // SAFETY: proc_pid_rusage initializes the complete rusage_info_v2 buffer
    // when it returns zero. The buffer is correctly sized and aligned.
    let result = unsafe {
        libc::proc_pid_rusage(
            std::process::id() as libc::c_int,
            libc::RUSAGE_INFO_V2,
            usage.as_mut_ptr().cast(),
        )
    };
    (result == 0).then(|| {
        // SAFETY: guarded by the successful proc_pid_rusage return above.
        unsafe { usage.assume_init().ri_phys_footprint }
    })
}

#[cfg(not(target_os = "macos"))]
fn current_physical_footprint() -> Option<u64> {
    None
}

fn format_ui_fps(fps: f64, sample_count: usize) -> String {
    if sample_count == 0 {
        "idle".to_owned()
    } else {
        format!("{fps:.1}")
    }
}

fn lock_frame_history(history: &Arc<Mutex<FrameHistory>>) -> MutexGuard<'_, FrameHistory> {
    history
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn format_frame_ms(milliseconds: f64, sample_count: usize) -> String {
    if sample_count == 0 {
        "warming up".to_owned()
    } else {
        format!("{milliseconds:.2} ms")
    }
}

fn format_cpu_percent(cpu_percent: Option<f32>) -> String {
    match cpu_percent {
        Some(percent) => format!("{percent:.1}%"),
        None => "warming up".to_owned(),
    }
}

fn format_rss_mib(rss_bytes: u64) -> String {
    format!("{:.1} MiB", rss_bytes as f64 / BYTES_PER_MIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_stats_compute_average_fps_and_p95() {
        let mut history = FrameHistory::new(8);
        for milliseconds in [10_u64, 12, 14, 16, 18] {
            history.push_frame_time(Duration::from_millis(milliseconds));
        }

        let stats = history.stats();
        assert_eq!(stats.sample_count, 5);
        assert!((stats.average_frame_ms - 14.0).abs() < 1e-9);
        assert!((stats.ui_fps - (1_000.0 / 14.0)).abs() < 0.001);
        assert!((stats.p95_frame_ms - 18.0).abs() < 1e-9);
    }

    #[test]
    fn frame_history_keeps_only_its_capacity() {
        let mut history = FrameHistory::new(3);
        for milliseconds in [10_u64, 20, 30, 40] {
            history.push_frame_time(Duration::from_millis(milliseconds));
        }

        let stats = history.stats();
        assert_eq!(stats.sample_count, 3);
        assert!((stats.average_frame_ms - 30.0).abs() < 1e-9);
    }

    #[test]
    fn formatters_include_units_and_warmup_state() {
        assert_eq!(format_ui_fps(60.0, 0), "idle");
        assert_eq!(format_ui_fps(59.94, 10), "59.9");
        assert_eq!(format_frame_ms(16.667, 10), "16.67 ms");
        assert_eq!(format_cpu_percent(None), "warming up");
        assert_eq!(format_cpu_percent(Some(127.34)), "127.3%");
        assert_eq!(format_rss_mib(64 * 1024 * 1024), "64.0 MiB");
    }
}
