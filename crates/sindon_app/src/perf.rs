//! Frame-timing instrumentation: where a painted frame's milliseconds went,
//! how many frames actually reached the screen in the last second, and how
//! much of the refresh-rate budget is left.
//!
//! # What "fps" means in an on-demand framework
//!
//! sindon paints on demand — a frame happens because an event arrived, an
//! animation voted for one, or a tick fired. An idle window paints *zero*
//! frames per second and that is the correct behaviour, not a stall. So the
//! throughput number ("frames painted in the last second") only means
//! something while something is continuously moving; the number that means
//! something *always* is [`FrameTimings::cpu`] — the work one frame costs.
//!
//! `cpu` is what decides whether the UI can hold the display's refresh rate:
//! at 60 Hz there are 16.7 ms per frame, so a 3 ms frame has ~13.7 ms of
//! headroom and could sustain ~330 fps if anything asked it to. It
//! deliberately excludes [`FrameTimings::acquire`] — the block inside
//! `get_current_texture()` — because that stall is the display pacing us,
//! not us being slow. A frame that is *ahead* of the display shows a large
//! `acquire`; a frame that is behind shows `acquire ≈ 0` and a large `cpu`.
//!
//! # Privacy
//!
//! Everything here is counts and durations. No text, no glyph identity, no
//! widget names — the same rule the `SINDON_PERF` log follows, so turning
//! instrumentation on can never turn a secret-aware app into a leaky one.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use sindon_core::{Color, Rect};
use sindon_widgets::paint::PaintContext;

/// Fallback frame budget — one 60 Hz refresh interval — used until the
/// window reports its monitor's real refresh rate, and on platforms where
/// winit can't report one.
///
/// The real rate matters more than it looks: on a 165 Hz display the budget
/// is 6.1 ms, so a 7 ms frame drops every other refresh while still looking
/// comfortably inside a 16.7 ms "60 Hz" budget. See
/// [`FrameRecorder::set_budget`].
pub const FRAME_BUDGET: Duration = Duration::from_micros(16_667);

/// How long the rolling window used by [`FrameRecorder::snapshot`] looks
/// back. One second, so the fps figure is literally "frames in the last
/// second" rather than a smoothed estimate.
const WINDOW: Duration = Duration::from_secs(1);

/// Ring capacity. At 60 fps this is ~10 s of history, which is far more than
/// the 1 s window needs — the slack is there so a burst of frames from a
/// high-refresh display still leaves a full window in the ring.
const RING_CAP: usize = 640;

/// One painted frame's cost, split by phase. All values are CPU-side wall
/// clock measured on the UI thread.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameTimings {
    /// Layout pass (`compute_layout_with_measure`, plus the hover resync's
    /// second pass on the rare frame that needs one). Text shaping performed
    /// through `measure` lands here.
    pub layout: Duration,
    /// Tree paint — walking the widgets and recording draw commands.
    pub paint: Duration,
    /// Atlas uploads, geometry build, command encoding and queue submit.
    pub encode: Duration,
    /// Blocked in `get_current_texture()` waiting for a free swapchain
    /// image. This is vsync back-pressure: the app is *ahead* of the
    /// display. Excluded from [`cpu`](Self::cpu).
    pub acquire: Duration,
    /// The `present()` call itself (queues the flip).
    pub present: Duration,
    /// Post-frame secure-atlas clear plus the `device.poll(Wait)` that
    /// guarantees the GPU finished it. A real stall the zeroize-first
    /// promise pays for, so it is counted in `cpu` — but only on frames
    /// that actually drew secure glyphs; it is zero on every other frame.
    pub sync: Duration,
    /// The perf HUD's own paint, when the overlay is on. Excluded from
    /// every other bucket *and* from [`cpu`](Self::cpu) so switching the
    /// HUD on doesn't inflate the numbers it displays — but it is reported
    /// separately so the cost of measuring stays visible.
    pub overlay: Duration,
    /// Text runs shaped this frame (cache misses only — a cached run costs
    /// a hash lookup and doesn't count).
    pub shapes: u32,
    /// Time spent inside those shaping calls.
    pub shape_time: Duration,
}

impl FrameTimings {
    /// What the frame actually cost: everything except the vsync block and
    /// the HUD's own paint. This is the number to compare against
    /// [`FRAME_BUDGET`].
    pub fn cpu(&self) -> Duration {
        self.layout + self.paint + self.encode + self.present + self.sync
    }

    /// Total wall clock from the start of the frame to the end of present,
    /// vsync block included. Equals `cpu + acquire + overlay`.
    pub fn wall(&self) -> Duration {
        self.cpu() + self.acquire + self.overlay
    }
}

/// Histogram bucket width. 0.25 ms resolution is finer than the jitter of
/// the underlying clock on a loaded desktop, so percentiles read from it are
/// limited by sample count, not by quantisation.
const BUCKET_MS: f64 = 0.25;
/// Bucket count: 0 – 64 ms at [`BUCKET_MS`] each, the last one absorbing
/// everything slower (a frame that far over budget is an outlier whose exact
/// value the percentiles don't need).
const BUCKETS: usize = 256;

/// Streaming summary of a set of frames: enough state to answer count, mean,
/// max and percentiles without keeping the samples.
///
/// Percentiles come from a fixed histogram rather than a sorted sample list
/// so a session summary costs the same after ten frames or ten million.
#[derive(Clone)]
struct Aggregate {
    count: u64,
    sum: Duration,
    max: Duration,
    /// Frames whose `cpu` exceeded [`FRAME_BUDGET`].
    slow: u64,
    hist: Box<[u32; BUCKETS]>,
}

impl Aggregate {
    fn new() -> Self {
        Self {
            count: 0,
            sum: Duration::ZERO,
            max: Duration::ZERO,
            slow: 0,
            hist: Box::new([0; BUCKETS]),
        }
    }

    fn record(&mut self, cpu: Duration, budget: Duration) {
        self.count += 1;
        self.sum += cpu;
        if cpu > self.max {
            self.max = cpu;
        }
        if cpu > budget {
            self.slow += 1;
        }
        let idx = ((cpu.as_secs_f64() * 1e3) / BUCKET_MS) as usize;
        self.hist[idx.min(BUCKETS - 1)] += 1;
    }

    fn mean_ms(&self) -> f32 {
        if self.count == 0 {
            return 0.0;
        }
        (self.sum.as_secs_f64() * 1e3 / self.count as f64) as f32
    }

    /// Approximate percentile in milliseconds, `q` in `0.0..=1.0`. Returns
    /// the upper edge of the bucket the rank falls in, so it never
    /// under-reports; `0.0` when no frames were recorded.
    ///
    /// Capped at the true maximum, because the bucket edge can otherwise
    /// exceed every sample that went into it — a run of 2.8 ms frames would
    /// print `p50=3.0 max=2.8` and read as a bug in the reader, not in the
    /// quantisation.
    fn percentile_ms(&self, q: f64) -> f32 {
        if self.count == 0 {
            return 0.0;
        }
        let max_ms = (self.max.as_secs_f64() * 1e3) as f32;
        // Rank is 1-based: q=1.0 must select the last recorded sample, not
        // one past it.
        let rank = (q * self.count as f64).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (i, n) in self.hist.iter().enumerate() {
            seen += *n as u64;
            if seen >= rank {
                return (((i + 1) as f64 * BUCKET_MS) as f32).min(max_ms);
            }
        }
        max_ms
    }
}

/// Rolling frame-timing recorder owned by the event loop: one `record` call
/// per painted frame, from which both the HUD and the `SINDON_PERF` log's
/// periodic summaries are derived.
pub struct FrameRecorder {
    /// Recent frames, newest last, capped at [`RING_CAP`]. Feeds the 1 s
    /// window; the aggregates below cover the whole session.
    ring: VecDeque<(Instant, FrameTimings)>,
    session: Aggregate,
    /// Frames since the last [`take_interval`](Self::take_interval), for the
    /// log's once-a-second summary line.
    interval: Aggregate,
    interval_start: Instant,
    /// One refresh interval of the display the window is on — the deadline a
    /// frame's `cpu` has to clear. [`FRAME_BUDGET`] until the event loop
    /// learns the real rate; see [`set_budget`](Self::set_budget).
    budget: Duration,
}

/// A closed one-second window, as handed to the `SINDON_PERF` log.
pub struct IntervalSummary {
    pub seconds: f32,
    pub frames: u64,
    pub fps: f32,
    pub mean_ms: f32,
    pub p50_ms: f32,
    pub p95_ms: f32,
    pub max_ms: f32,
    pub slow: u64,
}

/// What the HUD draws and what [`crate::FrameContext::perf`] hands to app
/// code: the last second condensed into scalars.
///
/// Phase figures are means over the window — a per-frame figure at 60 fps is
/// pure jitter, and the mean is what the budget comparison wants.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PerfSnapshot {
    /// Frames painted in the trailing second. Zero while idle — see the
    /// module docs.
    pub fps: f32,
    /// Mean [`FrameTimings::cpu`] over the window, in milliseconds.
    pub cpu_ms: f32,
    /// 95th percentile of `cpu` over the *session*, in milliseconds. Session
    /// rather than window because the interesting frame — the one that
    /// hitched — is rarely in the last second by the time it is read.
    pub cpu_p95_ms: f32,
    pub layout_ms: f32,
    pub paint_ms: f32,
    /// `encode + present`, in milliseconds — the GPU-facing CPU work, with
    /// the vsync block and the secure sync kept out.
    pub gpu_ms: f32,
    /// Mean time blocked on vsync ([`FrameTimings::acquire`]).
    pub wait_ms: f32,
    /// Mean secure-clear stall ([`FrameTimings::sync`]).
    pub sync_ms: f32,
    /// Mean HUD paint cost, in milliseconds; `0.0` with the overlay off.
    pub overlay_ms: f32,
    /// Mean text runs shaped per frame in the window.
    pub shapes: f32,
    /// Frames painted since the app started.
    pub session_frames: u64,
    /// Session frames whose `cpu` exceeded the budget in force when they
    /// were recorded.
    pub session_slow: u64,
    /// One refresh interval of the display the window is on, in
    /// milliseconds — the deadline `cpu_ms` is judged against. 16.7 until
    /// the real rate is known.
    pub budget_ms: f32,
}

impl PerfSnapshot {
    /// Milliseconds of the frame budget left unspent by the mean frame.
    /// Negative when the mean frame is over budget.
    pub fn headroom_ms(&self) -> f32 {
        self.budget_ms - self.cpu_ms
    }

    /// Frames per second the measured `cpu` cost could sustain if nothing
    /// throttled it. `f32::INFINITY` before the first frame is recorded.
    pub fn sustainable_fps(&self) -> f32 {
        if self.cpu_ms <= 0.0 {
            f32::INFINITY
        } else {
            1e3 / self.cpu_ms
        }
    }
}

impl FrameRecorder {
    pub fn new(now: Instant) -> Self {
        Self {
            ring: VecDeque::with_capacity(RING_CAP),
            session: Aggregate::new(),
            interval: Aggregate::new(),
            interval_start: now,
            budget: FRAME_BUDGET,
        }
    }

    /// Point the budget at the display's real refresh interval, so "slow
    /// frame" means "missed *this* monitor's deadline".
    ///
    /// The event loop calls this once the window is up, and again when the
    /// window moves to a monitor with a different rate. Frames already
    /// recorded keep the tally they were counted under — retroactively
    /// re-judging them would need every sample kept, and the aggregate
    /// deliberately doesn't keep them.
    pub fn set_budget(&mut self, budget: Duration) {
        self.budget = budget;
    }

    /// The deadline a frame's `cpu` currently has to clear.
    pub fn budget(&self) -> Duration {
        self.budget
    }

    /// Record one painted frame. `at` is the moment the frame finished
    /// presenting.
    pub fn record(&mut self, at: Instant, timings: FrameTimings) {
        let cpu = timings.cpu();
        self.session.record(cpu, self.budget);
        self.interval.record(cpu, self.budget);
        if self.ring.len() == RING_CAP {
            self.ring.pop_front();
        }
        self.ring.push_back((at, timings));
    }

    /// Frames painted since the app started.
    pub fn session_frames(&self) -> u64 {
        self.session.count
    }

    /// Condense the trailing second into a [`PerfSnapshot`].
    pub fn snapshot(&self, now: Instant) -> PerfSnapshot {
        let cutoff = now.checked_sub(WINDOW).unwrap_or(now);
        let window: Vec<&FrameTimings> = self
            .ring
            .iter()
            .rev()
            .take_while(|(at, _)| *at >= cutoff)
            .map(|(_, t)| t)
            .collect();

        let mut snap = PerfSnapshot {
            cpu_p95_ms: self.session.percentile_ms(0.95),
            session_frames: self.session.count,
            session_slow: self.session.slow,
            budget_ms: (self.budget.as_secs_f64() * 1e3) as f32,
            ..Default::default()
        };
        let n = window.len();
        if n == 0 {
            return snap;
        }
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        let mean = |total: f64| (total / n as f64) as f32;
        snap.fps = n as f32;
        snap.cpu_ms = mean(window.iter().map(|t| ms(t.cpu())).sum());
        snap.layout_ms = mean(window.iter().map(|t| ms(t.layout)).sum());
        snap.paint_ms = mean(window.iter().map(|t| ms(t.paint)).sum());
        snap.gpu_ms = mean(window.iter().map(|t| ms(t.encode) + ms(t.present)).sum());
        snap.wait_ms = mean(window.iter().map(|t| ms(t.acquire)).sum());
        snap.sync_ms = mean(window.iter().map(|t| ms(t.sync)).sum());
        snap.overlay_ms = mean(window.iter().map(|t| ms(t.overlay)).sum());
        snap.shapes = mean(window.iter().map(|t| t.shapes as f64).sum());
        snap
    }

    /// Close and drain the current interval once at least a second of wall
    /// clock has passed, for the log's periodic summary line. Returns `None`
    /// before that, and also when the elapsed second painted no frames at
    /// all — an idle app should produce silence, not a page of `fps=0`.
    pub fn take_interval(&mut self, now: Instant) -> Option<IntervalSummary> {
        let elapsed = now.duration_since(self.interval_start);
        if elapsed < WINDOW {
            return None;
        }
        let agg = std::mem::replace(&mut self.interval, Aggregate::new());
        self.interval_start = now;
        if agg.count == 0 {
            return None;
        }
        Some(IntervalSummary {
            seconds: elapsed.as_secs_f32(),
            frames: agg.count,
            fps: agg.count as f32 / elapsed.as_secs_f32(),
            mean_ms: agg.mean_ms(),
            p50_ms: agg.percentile_ms(0.5),
            p95_ms: agg.percentile_ms(0.95),
            max_ms: (agg.max.as_secs_f64() * 1e3) as f32,
            slow: agg.slow,
        })
    }

    /// Whole-session summary, for the line written on exit.
    pub fn session_summary(&self) -> IntervalSummary {
        IntervalSummary {
            seconds: 0.0,
            frames: self.session.count,
            fps: 0.0,
            mean_ms: self.session.mean_ms(),
            p50_ms: self.session.percentile_ms(0.5),
            p95_ms: self.session.percentile_ms(0.95),
            max_ms: (self.session.max.as_secs_f64() * 1e3) as f32,
            slow: self.session.slow,
        }
    }
}

/// The on-screen frame-timing overlay: a four-line readout pinned to the
/// window's top-right corner.
///
/// Painted by the event loop straight into the frame's [`PaintContext`]
/// after the tree — never a widget, so it can't be laid out, focused, hit-
/// tested or reached by a screen reader, and adding it can't perturb the
/// tree whose cost it reports.
pub struct PerfHud {
    /// The formatted lines, refreshed at [`HUD_REFRESH`] rather than every
    /// frame: at 60 fps a per-frame refresh would be unreadable *and* would
    /// miss the shape cache on every line, so the overlay would inflate the
    /// very numbers it prints.
    lines: Vec<String>,
    refreshed: Option<Instant>,
    /// Whether the last refresh saw a mean frame over budget — drives the
    /// readout colour.
    over_budget: bool,
}

/// How often the HUD re-formats its text. Fast enough to feel live, slow
/// enough that the strings stay in the shape cache between refreshes.
const HUD_REFRESH: Duration = Duration::from_millis(250);

const HUD_FONT_SIZE: f32 = 11.0;
const HUD_LINE_HEIGHT: f32 = 14.0;
const HUD_PAD: f32 = 6.0;
const HUD_MARGIN: f32 = 8.0;
const HUD_WIDTH: f32 = 176.0;

impl PerfHud {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            refreshed: None,
            over_budget: false,
        }
    }

    /// Draw the overlay for `snap` into the top-right corner of a window
    /// `viewport` logical pixels in size.
    ///
    /// Assumes the caller has already opened a fresh paint layer, so the
    /// readout lands above every widget layer including modals and toasts.
    pub fn paint(
        &mut self,
        snap: &PerfSnapshot,
        now: Instant,
        viewport: (f32, f32),
        ctx: &mut PaintContext,
    ) {
        if self
            .refreshed
            .is_none_or(|t| now.duration_since(t) >= HUD_REFRESH)
        {
            self.refresh(snap);
            self.refreshed = Some(now);
        }

        let height = HUD_PAD * 2.0 + HUD_LINE_HEIGHT * self.lines.len() as f32;
        let x = (viewport.0 - HUD_WIDTH - HUD_MARGIN).max(HUD_MARGIN);
        let panel = Rect::new(x, HUD_MARGIN, HUD_WIDTH, height);
        ctx.fill_rect_rounded(panel, Color::rgba(0.05, 0.06, 0.08, 0.82), 4.0);

        let text_color = if self.over_budget {
            Color::rgb(1.0, 0.62, 0.44)
        } else {
            Color::rgb(0.56, 0.94, 0.68)
        };

        for (i, line) in self.lines.iter().enumerate() {
            let shaped =
                ctx.text_engine
                    .shape_text(line, HUD_FONT_SIZE, HUD_LINE_HEIGHT, Some(HUD_WIDTH));
            let baseline_y = panel.origin.y + HUD_PAD + HUD_LINE_HEIGHT * i as f32;
            // First line is the headline number; the rest are the breakdown,
            // dimmed so the eye lands on the headline.
            let color = if i == 0 {
                text_color
            } else {
                Color::rgba(text_color.r, text_color.g, text_color.b, 0.72)
            };
            for glyph in &shaped.glyphs {
                if let Some(image) = ctx.text_engine.rasterize(glyph.cache_key) {
                    ctx.draw_glyph(
                        panel.origin.x + HUD_PAD + glyph.x,
                        baseline_y + glyph.y,
                        image,
                        color,
                        glyph.cache_key,
                    );
                }
            }
        }
    }

    fn refresh(&mut self, snap: &PerfSnapshot) {
        self.over_budget = snap.headroom_ms() < 0.0;
        self.lines.clear();
        self.lines.push(format!(
            "{:.0} fps   cpu {:.1}ms",
            snap.fps.round(),
            snap.cpu_ms
        ));
        self.lines.push(format!(
            "layout {:.1}  paint {:.1}",
            snap.layout_ms, snap.paint_ms
        ));
        self.lines.push(format!(
            "gpu {:.1}  sync {:.1}  wait {:.1}",
            snap.gpu_ms, snap.sync_ms, snap.wait_ms
        ));
        // p95 against the budget it has to clear, so the pair reads as one
        // fact ("2.8 of 6.1 ms") rather than two numbers to hold in mind.
        self.lines.push(format!(
            "p95 {:.1} / {:.1}ms  shapes {:.0}",
            snap.cpu_p95_ms, snap.budget_ms, snap.shapes
        ));
        self.lines.push(format!(
            "frames {}  slow {}  hud {:.1}",
            snap.session_frames, snap.session_slow, snap.overlay_ms
        ));
    }
}

impl Default for PerfHud {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timings(cpu_ms: f32) -> FrameTimings {
        FrameTimings {
            layout: Duration::from_secs_f32(cpu_ms * 1e-3 / 2.0),
            paint: Duration::from_secs_f32(cpu_ms * 1e-3 / 2.0),
            ..Default::default()
        }
    }

    #[test]
    fn cpu_excludes_vsync_block_and_overlay() {
        let t = FrameTimings {
            layout: Duration::from_millis(1),
            paint: Duration::from_millis(2),
            encode: Duration::from_millis(3),
            acquire: Duration::from_millis(12),
            present: Duration::from_millis(1),
            sync: Duration::from_millis(1),
            overlay: Duration::from_millis(5),
            ..Default::default()
        };
        assert_eq!(t.cpu(), Duration::from_millis(8));
        assert_eq!(t.wall(), Duration::from_millis(25));
    }

    #[test]
    fn fps_counts_only_the_trailing_second() {
        let start = Instant::now();
        let mut rec = FrameRecorder::new(start);
        // 30 frames spread over the last second, plus 10 older ones that
        // must not count toward fps.
        for i in 0..10 {
            rec.record(start + Duration::from_millis(i * 50), timings(2.0));
        }
        let now = start + Duration::from_secs(3);
        for i in 0..30 {
            rec.record(now - Duration::from_millis(999 - i * 30), timings(2.0));
        }
        let snap = rec.snapshot(now);
        assert_eq!(snap.fps, 30.0);
        assert_eq!(snap.session_frames, 40);
    }

    #[test]
    fn idle_window_reports_zero_fps_but_keeps_session_totals() {
        let start = Instant::now();
        let mut rec = FrameRecorder::new(start);
        for i in 0..5 {
            rec.record(start + Duration::from_millis(i * 10), timings(4.0));
        }
        // Nothing painted for ten seconds — the correct fps for an idle
        // on-demand UI is 0, and the session tally must survive it.
        let snap = rec.snapshot(start + Duration::from_secs(10));
        assert_eq!(snap.fps, 0.0);
        assert_eq!(snap.cpu_ms, 0.0);
        assert_eq!(snap.session_frames, 5);
    }

    #[test]
    fn phase_means_are_per_frame_not_totals() {
        let start = Instant::now();
        let mut rec = FrameRecorder::new(start);
        for i in 0..4 {
            rec.record(
                start + Duration::from_millis(i * 10),
                FrameTimings {
                    layout: Duration::from_millis(2),
                    paint: Duration::from_millis(3),
                    encode: Duration::from_millis(1),
                    present: Duration::from_millis(1),
                    acquire: Duration::from_millis(9),
                    shapes: 6,
                    ..Default::default()
                },
            );
        }
        let snap = rec.snapshot(start + Duration::from_millis(40));
        assert_eq!(snap.fps, 4.0);
        assert!((snap.layout_ms - 2.0).abs() < 0.01);
        assert!((snap.paint_ms - 3.0).abs() < 0.01);
        assert!((snap.gpu_ms - 2.0).abs() < 0.01, "encode + present");
        assert!((snap.wait_ms - 9.0).abs() < 0.01);
        assert!((snap.cpu_ms - 7.0).abs() < 0.01, "vsync block excluded");
        assert!((snap.shapes - 6.0).abs() < 0.01);
    }

    #[test]
    fn slow_frames_are_counted_against_the_60hz_budget() {
        let start = Instant::now();
        let mut rec = FrameRecorder::new(start);
        for i in 0..10 {
            let cpu = if i % 5 == 0 { 20.0 } else { 3.0 };
            rec.record(start + Duration::from_millis(i * 10), timings(cpu));
        }
        let snap = rec.snapshot(start + Duration::from_millis(100));
        assert_eq!(snap.session_slow, 2);
        assert_eq!(snap.session_frames, 10);
    }

    #[test]
    fn percentiles_track_the_tail() {
        let start = Instant::now();
        let mut rec = FrameRecorder::new(start);
        // 99 fast frames and one 40 ms hitch: p50 stays fast, p95 stays
        // fast, but the session max must show the hitch.
        for i in 0..99 {
            rec.record(start + Duration::from_millis(i), timings(2.0));
        }
        rec.record(start + Duration::from_millis(99), timings(40.0));
        let summary = rec.session_summary();
        assert!(summary.p50_ms <= 2.5, "p50 = {}", summary.p50_ms);
        assert!(summary.p95_ms <= 2.5, "p95 = {}", summary.p95_ms);
        assert!(summary.max_ms >= 39.0, "max = {}", summary.max_ms);
        assert_eq!(summary.slow, 1);
    }

    #[test]
    fn percentile_of_uniform_samples_lands_on_the_value() {
        let start = Instant::now();
        let mut rec = FrameRecorder::new(start);
        for i in 0..200 {
            rec.record(start + Duration::from_millis(i), timings(5.0));
        }
        let s = rec.session_summary();
        // Bucket upper edge, so within one bucket above the true value.
        assert!((s.p50_ms - 5.0).abs() <= BUCKET_MS as f32 + 0.01);
        assert!((s.p95_ms - 5.0).abs() <= BUCKET_MS as f32 + 0.01);
        assert!((s.mean_ms - 5.0).abs() < 0.05);
    }

    #[test]
    fn percentiles_never_exceed_the_recorded_max() {
        // A single 2.8 ms frame lands in the 2.75–3.0 bucket, whose upper
        // edge is above every sample recorded. Printing `p50=3.0 max=2.8`
        // reads as a broken reader, so percentiles are capped at the max.
        let start = Instant::now();
        let mut rec = FrameRecorder::new(start);
        rec.record(start, timings(2.8));
        let s = rec.session_summary();
        assert!(s.p50_ms <= s.max_ms, "p50 {} > max {}", s.p50_ms, s.max_ms);
        assert!(s.p95_ms <= s.max_ms, "p95 {} > max {}", s.p95_ms, s.max_ms);
        assert!(s.p50_ms <= s.p95_ms);
    }

    #[test]
    fn interval_closes_once_a_second_and_stays_silent_when_idle() {
        let start = Instant::now();
        let mut rec = FrameRecorder::new(start);
        rec.record(start + Duration::from_millis(100), timings(2.0));
        assert!(
            rec.take_interval(start + Duration::from_millis(500))
                .is_none()
        );

        let summary = rec
            .take_interval(start + Duration::from_millis(1_000))
            .expect("a second elapsed with one frame in it");
        assert_eq!(summary.frames, 1);

        // Second interval painted nothing: no line at all rather than fps=0
        // spam for every idle second.
        assert!(
            rec.take_interval(start + Duration::from_millis(2_000))
                .is_none()
        );
    }

    #[test]
    fn ring_is_bounded_but_session_totals_are_not() {
        let start = Instant::now();
        let mut rec = FrameRecorder::new(start);
        for i in 0..(RING_CAP as u64 + 500) {
            rec.record(start + Duration::from_millis(i), timings(1.0));
        }
        assert_eq!(rec.ring.len(), RING_CAP);
        assert_eq!(rec.session_frames(), RING_CAP as u64 + 500);
    }

    #[test]
    fn headroom_and_sustainable_fps_read_off_cpu_cost() {
        let snap = PerfSnapshot {
            cpu_ms: 4.0,
            budget_ms: 16.667,
            ..Default::default()
        };
        assert!((snap.headroom_ms() - 12.667).abs() < 0.01);
        assert!((snap.sustainable_fps() - 250.0).abs() < 0.01);

        let over = PerfSnapshot {
            cpu_ms: 25.0,
            budget_ms: 16.667,
            ..Default::default()
        };
        assert!(over.headroom_ms() < 0.0);
        assert!((over.sustainable_fps() - 40.0).abs() < 0.01);
    }

    #[test]
    fn a_high_refresh_display_shrinks_the_budget() {
        // 7 ms frames look fine against 60 Hz and miss every other refresh
        // on the 165 Hz panel this was first measured on.
        let start = Instant::now();
        let mut rec = FrameRecorder::new(start);
        assert_eq!(rec.budget(), FRAME_BUDGET);
        for i in 0..10 {
            rec.record(start + Duration::from_millis(i * 7), timings(7.0));
        }
        assert_eq!(rec.session_summary().slow, 0, "inside a 60 Hz budget");

        let mut rec = FrameRecorder::new(start);
        rec.set_budget(Duration::from_micros(6_060)); // 165 Hz
        for i in 0..10 {
            rec.record(start + Duration::from_millis(i * 7), timings(7.0));
        }
        assert_eq!(rec.session_summary().slow, 10, "over a 165 Hz budget");

        let snap = rec.snapshot(start + Duration::from_millis(70));
        assert!((snap.budget_ms - 6.06).abs() < 0.01);
        assert!(snap.headroom_ms() < 0.0);
    }
}
