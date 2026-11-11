//! Scripted interaction tests (`docs/13-testing-benchmarks.md` §6.1, TST-015):
//! every trace in `bench/traces/` (pointer, wheel and key input with
//! timestamps, as data) is replayed through the engine with a virtual clock
//! and a backend that draws nothing, and the events it produces, encoded as
//! the SDK receives them (`events::to_json`), must equal the `.events` file
//! beside it. Frames run at fixed 60 Hz times whenever the engine asks for
//! one, as `requestAnimationFrame` would run them.
//!
//! `CAVEA_BLESS=1 cargo test -p cavea-web traces` writes the `.events` files;
//! review their diff. A trace without one fails.
//!
//! A trace is `{ about, chart, viewport: [w, h], camera?: [x, y, w, h],
//! unavailable?: [[first idx, count]], max?, summary?, steps }`; each step is
//! `[t, op, …]`: `down`/`move`/`up`/`cancel` `id, "mouse"|"touch"|"pen", x,
//! y`; `wheel dx, dy, x, y, ctrl` (deltas in CSS pixels, `ctrl` 0 or 1);
//! `key name`; `focus 1|0` (`SetFocusShown`); `holdOk` and `holdFailed code`
//! (the answer to the oldest open hold request). Positions are CSS pixels.
//! With `summary`, the expected file holds each event type's count and last
//! event (camera changes per level of detail) instead of every event: the
//! 30-second benchmark trace (S2, `docs/13-testing-benchmarks.md` §6.2),
//! which the benchmark page replays with real time.

use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use cavea_render::{
    Engine,
    backend::{
        BackendError, BufferDesc, BufferId, Caps, DrawCall, GpuBackend, Rect as TexRect,
        ResourceId, Target, TextureDesc, TextureId,
    },
    protocol::{
        Bounds, CameraTarget, Command, Event, HoldOutcome, Modifiers, ObjectFailure, ObjectRef,
        PointerKind, PointerType,
    },
};
use serde_json::Value;

/// One display frame at 60 Hz, in milliseconds.
const FRAME_MS: f64 = 1000.0 / 60.0;
/// After the last step, frames run at most this long; still animating then fails.
const SETTLE_MS: f64 = 10_000.0;

/// A backend that draws nothing: the events are the engine's alone.
#[derive(Default)]
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
    fn update_buffer(&mut self, _: BufferId, _: u32, _: &[u8]) -> Result<(), BackendError> {
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
    fn update_texture(&mut self, _: TextureId, _: TexRect, _: &[u8]) -> Result<(), BackendError> {
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

fn traces_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/traces")
}

/// The engine, the virtual clock and what it has said.
struct Replay {
    engine: Engine<NoGpu>,
    /// The frame the engine asked for, if any (a frame number at 60 Hz).
    frame_wanted: Option<u64>,
    /// Hold requests not yet answered, oldest first.
    holds: VecDeque<u32>,
    /// Events with the time they came out at.
    log: Vec<(f64, Event)>,
}

impl Replay {
    fn send(&mut self, t: f64, command: Command) {
        self.engine.handle(command);
        for event in self.engine.drain_events().collect::<Vec<_>>() {
            match event {
                Event::NeedFrame => {
                    // The next display frame strictly after now (a frame's
                    // own time divides back to just under its number).
                    let next = (t / FRAME_MS + 1e-6).floor() as u64 + 1;
                    self.frame_wanted.get_or_insert(next);
                }
                Event::Error { code, message } => {
                    panic!("engine error at {t} ms: {code:?} {message}")
                }
                Event::HoldRequest { request, .. } => {
                    self.holds.push_back(request);
                    self.log.push((t, event));
                }
                // Text is drawn by the host; chart metadata is not interaction.
                Event::TextRequest { .. } | Event::Ready(_) => {}
                event => self.log.push((t, event)),
            }
        }
    }

    /// Runs the frames the engine asks for up to `until` ms.
    fn frames_until(&mut self, until: f64) {
        while let Some(frame) = self.frame_wanted {
            let t = frame as f64 * FRAME_MS;
            if t > until {
                break;
            }
            self.frame_wanted = None;
            self.send(t, Command::Tick { time_ms: t });
        }
    }
}

fn num(v: Option<&Value>, what: &str) -> f64 {
    v.and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a number for {what}"))
}

fn text<'a>(v: Option<&'a Value>, what: &str) -> &'a str {
    v.and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string for {what}"))
}

fn chart(name: &str) -> Vec<u8> {
    let preset = cavea_testkit::presets::find(name).unwrap_or_else(|| panic!("no preset {name}"));
    let compiled = cavea_compiler::compile(&preset.generate(cavea_testkit::presets::DEFAULT_SEED));
    compiled.expect("the preset compiles").public
}

/// Replays `trace` and returns its events, one line each.
fn replay(trace: &Value, charts: &mut BTreeMap<String, Vec<u8>>) -> String {
    let name = text(trace.get("chart"), "chart");
    let ccf = charts
        .entry(name.to_owned())
        .or_insert_with(|| chart(name))
        .clone();
    let size = |i| num(trace["viewport"].get(i), "viewport") as f32;
    let mut r = Replay {
        engine: Engine::new(NoGpu::default()),
        frame_wanted: None,
        holds: VecDeque::new(),
        log: Vec::new(),
    };
    r.send(
        0.0,
        Command::Resize {
            width: size(0),
            height: size(1),
            dpr: 1.0,
            max_dpr: 3.0,
        },
    );
    r.send(0.0, Command::LoadChart { ccf });
    let objects = r.engine.objects().expect("the chart loaded");
    let mut state = vec![0; objects];
    for run in trace["unavailable"].as_array().into_iter().flatten() {
        let first = num(run.get(0), "unavailable") as usize;
        let count = num(run.get(1), "unavailable") as usize;
        state[first..first + count].fill(1);
    }
    r.send(0.0, Command::SetSnapshot { seq: 1, state });
    if let Some(max) = trace["max"].as_u64() {
        let max = Some(max as u32);
        let (min, validators, group_size) = (None, Vec::new(), None);
        r.send(
            0.0,
            Command::SetRules {
                max,
                min,
                validators,
                group_size,
            },
        );
    }
    if let Some(camera) = trace["camera"].as_array() {
        let at = |i| num(camera.get(i), "camera") as f32;
        let target = CameraTarget::Bounds(Bounds {
            x: at(0),
            y: at(1),
            width: at(2),
            height: at(3),
        });
        let duration_ms = Some(0);
        r.send(
            0.0,
            Command::FlyTo {
                target,
                duration_ms,
            },
        );
    }
    r.frame_wanted = None;
    r.send(0.0, Command::Tick { time_ms: 0.0 });

    let mut last = 0.0;
    for step in trace["steps"].as_array().expect("steps").iter() {
        let step = step.as_array().expect("a step is an array");
        let t = num(step.first(), "time");
        assert!(t >= last, "steps go back in time at {t} ms");
        last = t;
        r.frames_until(t);
        let op = text(step.get(1), "op");
        let arg = |i: usize| step.get(i);
        let command = match op {
            "down" | "move" | "up" | "cancel" => Command::Pointer {
                kind: match op {
                    "down" => PointerKind::Down,
                    "move" => PointerKind::Move,
                    "up" => PointerKind::Up,
                    _ => PointerKind::Cancel,
                },
                id: num(arg(2), "pointer id") as u32,
                pointer_type: match text(arg(3), "pointer type") {
                    "mouse" => PointerType::Mouse,
                    "touch" => PointerType::Touch,
                    _ => PointerType::Pen,
                },
                x: num(arg(4), "x") as f32,
                y: num(arg(5), "y") as f32,
                time_ms: t,
            },
            "wheel" => Command::Wheel {
                dx: num(arg(2), "dx") as f32,
                dy: num(arg(3), "dy") as f32,
                x: num(arg(4), "x") as f32,
                y: num(arg(5), "y") as f32,
                modifiers: Modifiers {
                    ctrl: num(arg(6), "ctrl") != 0.0,
                    ..Modifiers::default()
                },
                time_ms: t,
            },
            "key" => Command::Key {
                key: text(arg(2), "key").to_owned(),
                modifiers: Modifiers::default(),
            },
            "focus" => Command::SetFocusShown(num(arg(2), "focus") != 0.0),
            "holdOk" | "holdFailed" => {
                let request = r.holds.pop_front().expect("an open hold request");
                let outcome = if op == "holdOk" {
                    HoldOutcome::Ok
                } else {
                    HoldOutcome::Failed(vec![ObjectFailure {
                        object: ObjectRef::Label(String::new()),
                        code: text(arg(2), "code").to_owned(),
                    }])
                };
                Command::HoldResult { request, outcome }
            }
            _ => panic!("unknown op {op}"),
        };
        r.send(t, command);
    }
    r.frames_until(last + SETTLE_MS);
    assert!(
        r.frame_wanted.is_none(),
        "still drawing {SETTLE_MS} ms after the last step"
    );

    let line = |out: &mut String, t: f64, event: &Event| {
        let mut json = String::new();
        crate::events::to_json(std::slice::from_ref(event), &mut json);
        let _ = writeln!(out, "{t:9.1} {}", &json[1..json.len() - 1]);
    };
    let mut out = String::new();
    if trace["summary"].as_bool() == Some(true) {
        let mut types: BTreeMap<String, (usize, f64, &Event)> = BTreeMap::new();
        for (t, event) in &r.log {
            let mut json = String::new();
            crate::events::to_json(std::slice::from_ref(event), &mut json);
            // By type, and camera changes by level, so each level reached shows.
            let mut kind = json.split('"').nth(3).unwrap_or_default().to_owned();
            if let Event::CameraChanged { lod, .. } = event {
                let _ = write!(kind, " {lod:?}");
            }
            let entry = types.entry(kind).or_insert((0, *t, event));
            *entry = (entry.0 + 1, *t, event);
        }
        for (kind, (count, t, event)) in types {
            let _ = writeln!(out, "{count} {kind}, the last:");
            line(&mut out, t, event);
        }
    } else {
        for (t, event) in &r.log {
            line(&mut out, *t, event);
        }
    }
    out
}

#[test]
fn traces_produce_their_events() {
    let bless = std::env::var_os("CAVEA_BLESS").is_some();
    let mut charts = BTreeMap::new();
    let mut paths: Vec<PathBuf> = fs::read_dir(traces_dir())
        .expect("bench/traces")
        .map(|e| e.expect("a directory entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no traces in bench/traces");
    let mut failures = Vec::new();
    for path in &paths {
        let trace: Value = serde_json::from_str(&fs::read_to_string(path).expect("readable"))
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let actual = replay(&trace, &mut charts);
        let expected_path = path.with_extension("events");
        if bless {
            fs::write(&expected_path, &actual).expect("writable");
            continue;
        }
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let Ok(expected) = fs::read_to_string(&expected_path) else {
            failures.push(format!(
                "{name}: no {}; write it with CAVEA_BLESS=1",
                expected_path.display()
            ));
            continue;
        };
        if expected != actual {
            let (n, (want, got)) = expected
                .lines()
                .chain(std::iter::repeat("(nothing)"))
                .zip(actual.lines().chain(std::iter::repeat("(nothing)")))
                .enumerate()
                .find(|(_, (a, b))| a != b)
                .unwrap_or((0, ("", "")));
            failures.push(format!(
                "{name}: first difference at line {}\n  expected {want}\n  actual   {got}\n  guards: {}",
                n + 1,
                trace["about"].as_str().unwrap_or_default()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
