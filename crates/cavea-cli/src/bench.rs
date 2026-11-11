//! `cavea bench`: the micro-benchmarks of `docs/13-testing-benchmarks.md`
//! §6.3, on the `stadium-100k` preset unless named otherwise.
//!
//! Each benchmark warms up, then takes [`SAMPLES`] samples of at least
//! [`SAMPLE_NS`] each (as many runs as fit) and reports the median and the
//! minimum time per run. Noise is handled by the median and by batching short
//! runs; a regression is flagged only beyond [`SLOWER_PERCENT`] of a baseline
//! measured on the same machine, never against a number from elsewhere.

use std::hint::black_box;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, ensure};
use cavea_format::{Artefact, CHUNK_MAX_SEATS, wire};
use cavea_render::backend::{
    BackendError, BufferDesc, BufferId, Caps, DrawCall, GpuBackend, Rect, ResourceId, Target,
    TextureDesc, TextureId,
};
use cavea_render::focus::{self, Dir};
use cavea_render::protocol::{CameraTarget, Command, Event, Modifiers, ObjectRef};
use cavea_render::{Engine, hit::hit_test, scene::Scene};
use cavea_testkit::presets::{self, DEFAULT_SEED, PRESETS};
use serde_json::{Value, json};

/// The benchmarks, in run order. `compile` covers every preset.
pub const NAMES: [&str; 7] = [
    "decode",
    "hit-test",
    "focus",
    "snapshot-apply",
    "delta-apply",
    "label-build",
    "compile",
];
const SAMPLES: usize = 15;
const SAMPLE_NS: f64 = 10e6;
/// Sampling stops after this long once it has [`MIN_SAMPLES`].
const SAMPLING_NS: f64 = 3e9;
const MIN_SAMPLES: usize = 5;
/// More than this much slower than the baseline is a regression (13 §6.3).
pub const SLOWER_PERCENT: f64 = 10.0;
const PRESET: &str = "stadium-100k";

/// One benchmark's result: times per run in nanoseconds.
pub struct Measured {
    pub name: String,
    /// What one run is.
    pub what: String,
    pub median_ns: f64,
    pub min_ns: f64,
    pub samples: usize,
    /// A budget with a hard number and where it is stated.
    pub budget: Option<(f64, &'static str)>,
}

impl Measured {
    pub fn over_budget(&self) -> bool {
        self.budget.is_some_and(|(ns, _)| self.median_ns > ns)
    }
}

/// Samples `run`, which performs one run and returns its time.
fn sample(mut run: impl FnMut() -> Duration) -> (f64, f64, usize) {
    let warm = Instant::now();
    while warm.elapsed().as_secs_f64() < 0.2 {
        run();
    }
    let mut per_run = Vec::with_capacity(SAMPLES);
    let mut total = 0.0;
    while per_run.len() < SAMPLES && (per_run.len() < MIN_SAMPLES || total < SAMPLING_NS) {
        let (mut ns, mut runs) = (0.0, 0u32);
        while ns < SAMPLE_NS {
            ns += run().as_secs_f64() * 1e9;
            runs += 1;
        }
        total += ns;
        per_run.push(ns / f64::from(runs));
    }
    per_run.sort_by(f64::total_cmp);
    let median = per_run.get(per_run.len() / 2).copied().unwrap_or(f64::NAN);
    let min = per_run.first().copied().unwrap_or(f64::NAN);
    (median, min, per_run.len())
}

/// Times one call of `f`.
fn timed<T>(f: impl FnOnce() -> T) -> Duration {
    let start = Instant::now();
    black_box(f());
    start.elapsed()
}

/// xorshift64*: the same points on every run, no dependency.
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, n: usize) -> usize {
        (self.unit() * n as f64) as usize
    }
}

/// A backend that does nothing: the benchmarks measure the engine's work.
struct NoGpu(u32);

impl GpuBackend for NoGpu {
    fn caps(&self) -> Caps {
        Caps {
            max_texture_size: 4096,
        }
    }
    fn create_buffer(&mut self, _: BufferDesc, _: &[u8]) -> Result<BufferId, BackendError> {
        self.0 += 1;
        Ok(BufferId(self.0))
    }
    fn update_buffer(&mut self, _: BufferId, _: u32, data: &[u8]) -> Result<(), BackendError> {
        black_box(data);
        Ok(())
    }
    fn create_texture(
        &mut self,
        _: TextureDesc,
        _: Option<&[u8]>,
    ) -> Result<TextureId, BackendError> {
        self.0 += 1;
        Ok(TextureId(self.0))
    }
    fn update_texture(&mut self, _: TextureId, _: Rect, _: &[u8]) -> Result<(), BackendError> {
        Ok(())
    }
    fn begin_frame(&mut self, _: Target, _: [f32; 4]) {}
    fn draw(&mut self, _: &DrawCall<'_>) -> Result<(), BackendError> {
        Ok(())
    }
    fn end_frame(&mut self) {}
    fn destroy(&mut self, _: ResourceId) {}
    fn restore(&mut self) -> Result<(), BackendError> {
        Ok(())
    }
}

/// Runs the benchmarks in `names` (all for none), in [`NAMES`] order.
pub fn run(names: &[&str], mut progress: impl FnMut(&str)) -> Result<Vec<Measured>> {
    let wanted = |name: &str| names.is_empty() || names.contains(&name);
    let mut out = Vec::new();
    let mut push = |out: &mut Vec<Measured>,
                    name: &str,
                    what: String,
                    budget: Option<(f64, &'static str)>,
                    (median_ns, min_ns, samples): (f64, f64, usize)| {
        progress(name);
        out.push(Measured {
            name: name.to_owned(),
            what,
            median_ns,
            min_ns,
            samples,
            budget,
        });
    };
    let preset = presets::find(PRESET).context("no stadium-100k preset")?;
    let ccf = cavea_compiler::compile(&preset.generate(DEFAULT_SEED))
        .context("stadium-100k does not compile")?
        .public;
    let chart =
        cavea_format::decode(&ccf, Artefact::Public).context("stadium-100k does not decode")?;
    let seats = chart.seat_pos.len();

    if wanted("decode") {
        let budget = Some((5e6 * seats as f64 / 1e5, "03 §4.4"));
        let m = sample(|| timed(|| cavea_format::decode(&ccf, Artefact::Public)));
        push(
            &mut out,
            "decode",
            format!("{seats} seats, {} B", ccf.len()),
            budget,
            m,
        );
    }
    if wanted("hit-test") {
        let scene = Scene::decode(&ccf).context("stadium-100k scene")?;
        let mut rng = Rng(7);
        // Near seats, at 18 to 64 px seats (the zooms that select), with the
        // touch tolerance of 12 px.
        let points: Vec<([f32; 2], f32)> = (0..10_000)
            .map(|_| {
                let [x, y] = scene
                    .pos
                    .get(rng.below(scene.pos.len()))
                    .copied()
                    .unwrap_or_default();
                let px_per_cu = (18.0 + 46.0 * rng.unit()) / f64::from(scene.seat_size);
                let jitter = |rng: &mut Rng| (rng.unit() * 60.0 - 30.0) as f32;
                (
                    [x + jitter(&mut rng), y + jitter(&mut rng)],
                    (12.0 / px_per_cu) as f32,
                )
            })
            .collect();
        let n = points.len() as f64;
        let (median, min, samples) = sample(|| {
            timed(|| {
                points
                    .iter()
                    .filter(|(p, t)| hit_test(&scene, *p, *t).is_some())
                    .count()
            })
        });
        let what = format!("{seats} seats, a tap near a seat at 18 to 64 px");
        push(
            &mut out,
            "hit-test",
            what,
            Some((20e3, "04 §15")),
            (median / n, min / n, samples),
        );
    }
    if wanted("focus") {
        let scene = Scene::decode(&ccf).context("stadium-100k scene")?;
        let mut rng = Rng(5);
        let dirs = [Dir::Left, Dir::Right, Dir::Up, Dir::Down];
        let cases: Vec<(u32, Dir)> = (0..10_000)
            .map(|_| {
                let dir = dirs.get(rng.below(4)).copied().unwrap_or(Dir::Right);
                (rng.below(scene.pos.len()) as u32, dir)
            })
            .collect();
        let n = cases.len() as f64;
        let (median, min, samples) = sample(|| {
            timed(|| {
                (cases.iter())
                    .filter(|&&(from, dir)| focus::step(&scene, from, dir).is_some())
                    .count()
            })
        });
        let what = format!("{seats} seats, an arrow key's search from a random seat");
        push(
            &mut out,
            "focus-step",
            what,
            Some((20e3, "11 §3")),
            (median / n, min / n, samples),
        );
    }
    let engine = || {
        let mut e = Engine::new(NoGpu(0));
        e.handle(Command::LoadChart { ccf: ccf.clone() });
        e.handle(Command::Resize {
            width: 1280.0,
            height: 800.0,
            dpr: 1.0,
            max_dpr: 2.0,
        });
        e.handle(Command::Tick { time_ms: 0.0 });
        e
    };
    // Applies a frame and draws the next frame; false if the engine refused
    // the frame or asked for a resync, so nothing was measured.
    let mut clock = 0.0;
    let mut apply = |e: &mut Engine<NoGpu>, frame: &[u8], objects: Option<u32>| {
        let Ok(Some(command)) = Command::from_frame(frame, objects) else {
            return false;
        };
        e.handle(command);
        clock += 16.0;
        e.handle(Command::Tick { time_ms: clock });
        !e.drain_events()
            .any(|ev| matches!(ev, Event::Error { .. } | Event::Resync { .. }))
    };
    if wanted("focus") {
        // An arrow key through the engine: the search, the camera's follow
        // and the event. At overview the follow zooms in; the camera is put
        // back between runs, untimed.
        for (name, overview) in [("focus-key overview", true), ("focus-key detail", false)] {
            let mut e = engine();
            e.handle(Command::SetReducedMotion(true));
            let target = if overview {
                let b = e.chart_bounds().context("no chart bounds")?;
                CameraTarget::Bounds(b)
            } else {
                CameraTarget::Object(ObjectRef::Idx(0))
            };
            e.handle(Command::SetCamera {
                target: target.clone(),
            });
            e.handle(Command::Tick { time_ms: 0.0 });
            let mut clock = 0.0;
            let key = |e: &mut Engine<NoGpu>| {
                e.handle(Command::Key {
                    key: "ArrowRight".to_owned(),
                    modifiers: Modifiers::default(),
                });
                e.drain_events().count()
            };
            key(&mut e);
            let m = sample(|| {
                if overview {
                    e.handle(Command::SetCamera {
                        target: target.clone(),
                    });
                    clock += 16.0;
                    e.handle(Command::Tick { time_ms: clock });
                    e.drain_events().for_each(drop);
                }
                timed(|| key(&mut e))
            });
            let what = "an arrow key: search, camera, event".to_owned();
            push(&mut out, name, what, Some((50e3, "11 §3")), m);
        }
    }
    let mut rng = Rng(11);
    let state: Vec<u8> = (0..seats).map(|_| u8::from(rng.unit() < 0.3)).collect();
    if wanted("snapshot-apply") {
        let mut e = engine();
        let objects = e.objects().and_then(|n| u32::try_from(n).ok());
        let frame =
            wire::encode_snapshot(wire::View::Public, 1, &state, &[]).context("snapshot")?;
        let mut ok = true;
        let m = sample(|| timed(|| ok &= apply(&mut e, &frame, objects)));
        ensure!(ok, "the engine refused the snapshot");
        let what = format!(
            "{seats} objects ({} B frame): decode, apply, upload",
            frame.len()
        );
        push(&mut out, "snapshot-apply", what, None, m);
    }
    if wanted("delta-apply") {
        let mut e = engine();
        let objects = e.objects().and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
        let snapshot =
            wire::encode_snapshot(wire::View::Public, 1, &state, &[]).context("snapshot")?;
        ensure!(apply(&mut e, &snapshot, Some(objects)), "snapshot refused");
        // 10,000 distinct seats spread over the chart, flipped each run.
        let mut idx: Vec<u32> = (0..10_000).map(|_| rng.below(seats) as u32).collect();
        idx.sort_unstable();
        idx.dedup();
        while idx.len() < 10_000 {
            idx.push(rng.below(seats) as u32);
            idx.sort_unstable();
            idx.dedup();
        }
        let (mut seq, mut ok) = (1, true);
        let m = sample(|| {
            let changes: Vec<wire::Change> = idx
                .iter()
                .map(|&i| wire::Change {
                    idx: i,
                    code: u8::from(seq % 2 == 1) ^ state.get(i as usize).copied().unwrap_or(0),
                })
                .collect();
            let frame =
                wire::encode_delta(wire::View::Public, seq, seq + 1, &changes, objects, &[])
                    .unwrap_or_default();
            seq += 1;
            timed(|| ok &= apply(&mut e, &frame, Some(objects)))
        });
        ensure!(ok, "the engine refused a delta");
        let what = "10,000 changes: decode, apply, upload".to_owned();
        push(&mut out, "delta-apply", what, Some((2e6, "01 §5")), m);
    }
    if wanted("label-build") {
        // A full render chunk's worth (03 §4.2: at most 4,096 seats); the
        // stadium's sections are smaller, so the seats span a few chunks.
        let seats = CHUNK_MAX_SEATS.min(chart.meta.seat_count);
        let labels = || {
            (0..seats)
                .filter_map(|i| cavea_format::full_label::seat_label(&chart, i).ok())
                .inspect(|l| {
                    black_box(l.seat);
                })
                .count()
        };
        ensure!(labels() == seats as usize, "a seat has no label");
        let m = sample(|| timed(labels));
        let what = format!("seat labels of {seats} seats, a full render chunk's worth");
        push(&mut out, "label-build", what, Some((1e6, "04 §14")), m);
    }
    if wanted("compile") {
        for p in &PRESETS {
            let doc = p.generate(DEFAULT_SEED);
            cavea_compiler::compile(&doc)
                .with_context(|| format!("{} does not compile", p.name))?;
            let m = sample(|| timed(|| cavea_compiler::compile(&doc)));
            push(
                &mut out,
                &format!("compile {}", p.name),
                format!("{} seats, both artefacts", p.seats),
                None,
                m,
            );
        }
    }
    Ok(out)
}

/// A later run against a baseline: the change in percent, and whether it is
/// a regression (more than [`SLOWER_PERCENT`] slower).
pub fn compare(baseline_ns: f64, now_ns: f64) -> (f64, bool) {
    let change = (now_ns / baseline_ns - 1.0) * 100.0;
    // Products, not the rounded `change`: exactly 10% slower is not flagged.
    (
        change,
        now_ns * 100.0 > baseline_ns * (100.0 + SLOWER_PERCENT),
    )
}

/// The baseline file: each benchmark's median per run, in nanoseconds.
pub fn to_baseline(results: &[Measured]) -> Value {
    let medians: serde_json::Map<String, Value> = results
        .iter()
        .map(|m| (m.name.clone(), json!(m.median_ns)))
        .collect();
    json!({ "cavea-bench": 1, "medianNs": medians })
}

/// Reads a baseline written by [`to_baseline`]: benchmark name to median.
pub fn from_baseline(value: &Value) -> Option<Vec<(String, f64)>> {
    (value.get("cavea-bench")? == 1).then_some(())?;
    let medians = value.get("medianNs")?.as_object()?;
    medians
        .iter()
        .map(|(k, v)| Some((k.clone(), v.as_f64()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regressions_are_more_than_ten_percent_slower() {
        let slower = |base, now| compare(base, now).1;
        assert!(slower(100.0, 110.5), "10.5% slower is flagged");
        assert!(!slower(100.0, 110.0), "exactly 10% is not");
        assert!(!slower(100.0, 107.0), "noise under the threshold is not");
        assert!(!slower(100.0, 60.0), "faster is not");
        assert!(slower(0.5, 0.6), "units do not matter");
        let (change, _) = compare(200.0, 150.0);
        assert!((change + 25.0).abs() < 1e-9);
    }
}
