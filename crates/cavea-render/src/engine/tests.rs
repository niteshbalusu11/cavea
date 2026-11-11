//! Engine behaviour driven only through `Engine::handle` with scripted,
//! timestamped commands, as a recorded input trace would drive it (04 §26).

use std::collections::BTreeMap;

use super::*;
use crate::{
    backend::Caps,
    protocol::{GestureHandling, Modifiers},
};

mod chart;
mod context;
mod keys;
mod labels;
mod overview;
mod select;
mod state;
mod text;
use PointerKind::{Cancel, Down, Move, Up};

const MOUSE: PointerType = PointerType::Mouse;
const TOUCH: PointerType = PointerType::Touch;
const W: f32 = 800.0;
const H: f32 = 600.0;
const SIZE: Size = Size {
    width: 800.0,
    height: 600.0,
};

/// A backend that keeps every live resource's bytes and records every call.
#[derive(Default)]
struct Fake {
    next: u32,
    buffers: BTreeMap<u32, Vec<u8>>,
    textures: BTreeMap<u32, (TextureDesc, Vec<u8>)>,
    /// `update_buffer` calls: buffer, byte offset, length.
    buffer_writes: Vec<(u32, u32, usize)>,
    texture_writes: Vec<(u32, TexRect)>,
    destroyed: Vec<ResourceId>,
    /// Instance ranges of each `shape` draw: seats and overlays.
    draws: Vec<Vec<Range<u32>>>,
    /// Every draw in order: pipeline, ranges and pass opacity.
    calls: Vec<(Pipeline, Vec<Range<u32>>, f32)>,
    /// The dim factor of each `draw`.
    dims: Vec<f32>,
    /// Ranges and label buffer bases of each `glyph` draw of seat labels.
    glyph_draws: Vec<(Vec<Range<u32>>, Vec<u32>)>,
    /// Every `glyph` draw of row labels and `text` draw: pipeline, buffer,
    /// ranges, opacity.
    text_draws: Vec<(Pipeline, u32, Vec<Range<u32>>, f32)>,
    /// The mode of each `shape` draw.
    modes: Vec<f32>,
    targets: Vec<Target>,
    clears: Vec<[f32; 4]>,
    restores: u32,
    fail_restore: bool,
}

type E = Engine<Fake>;

impl GpuBackend for Fake {
    fn caps(&self) -> Caps {
        Caps {
            max_texture_size: 4096,
        }
    }
    fn create_buffer(&mut self, _: BufferDesc, data: &[u8]) -> Result<BufferId, BackendError> {
        self.next += 1;
        self.buffers.insert(self.next, data.to_vec());
        Ok(BufferId(self.next))
    }
    fn update_buffer(
        &mut self,
        id: BufferId,
        offset: u32,
        data: &[u8],
    ) -> Result<(), BackendError> {
        let buffer = self
            .buffers
            .get_mut(&id.0)
            .ok_or(BackendError::UnknownResource)?;
        let slot = buffer
            .get_mut(offset as usize..offset as usize + data.len())
            .ok_or(BackendError::InvalidData)?;
        slot.copy_from_slice(data);
        self.buffer_writes.push((id.0, offset, data.len()));
        Ok(())
    }
    fn create_texture(
        &mut self,
        desc: TextureDesc,
        data: Option<&[u8]>,
    ) -> Result<TextureId, BackendError> {
        self.next += 1;
        let data = data.map(<[u8]>::to_vec).unwrap_or_default();
        self.textures.insert(self.next, (desc, data));
        Ok(TextureId(self.next))
    }
    fn update_texture(
        &mut self,
        id: TextureId,
        region: TexRect,
        data: &[u8],
    ) -> Result<(), BackendError> {
        let (desc, bytes) = self
            .textures
            .get_mut(&id.0)
            .ok_or(BackendError::UnknownResource)?;
        let bpp = desc.format.bytes_per_pixel() as usize;
        if region.x + region.width > desc.width
            || region.y + region.height > desc.height
            || data.len() != (region.width * region.height) as usize * bpp
        {
            return Err(BackendError::InvalidData);
        }
        bytes.resize((desc.width * desc.height) as usize * bpp, 0);
        for (row, line) in data.chunks(region.width as usize * bpp).enumerate() {
            let at = ((region.y as usize + row) * desc.width as usize + region.x as usize) * bpp;
            bytes[at..at + line.len()].copy_from_slice(line);
        }
        self.texture_writes.push((id.0, region));
        Ok(())
    }
    fn begin_frame(&mut self, target: Target, clear: [f32; 4]) {
        self.targets.push(target);
        self.clears.push(clear);
    }
    fn draw(&mut self, call: &DrawCall<'_>) -> Result<(), BackendError> {
        let ranges = call.ranges.to_vec();
        self.calls
            .push((call.pipeline, ranges.clone(), call.uniforms.alpha));
        let buffer = call.buffers[0].map_or(0, |b| b.0);
        match call.pipeline {
            Pipeline::Shape => {
                self.draws.push(ranges);
                self.dims.push(call.uniforms.dim);
                self.modes.push(call.uniforms.mode);
            }
            Pipeline::Glyph if call.uniforms.mode == crate::backend::mode::SEAT_ICONS => {
                self.glyph_draws.push((ranges, call.bases.to_vec()));
            }
            Pipeline::Mesh => {}
            pipeline => {
                let alpha = call.uniforms.alpha;
                self.text_draws.push((pipeline, buffer, ranges, alpha));
            }
        }
        Ok(())
    }
    fn end_frame(&mut self) {}
    fn restore(&mut self) -> Result<(), BackendError> {
        self.restores += 1;
        // Every resource of the lost context is gone.
        self.buffers.clear();
        self.textures.clear();
        if self.fail_restore {
            Err(BackendError::Create("context".to_owned()))
        } else {
            Ok(())
        }
    }
    fn destroy(&mut self, id: ResourceId) {
        match id {
            ResourceId::Buffer(b) => self.buffers.remove(&b.0).map(drop),
            ResourceId::Texture(t) => self.textures.remove(&t.0).map(drop),
        };
        self.destroyed.push(id);
    }
}

/// Public CCF bytes of a chart document given as JSON.
fn compile(json: &str) -> Vec<u8> {
    let doc = cavea_compiler::ChartDocument::from_json(json.as_bytes()).unwrap();
    cavea_compiler::compile(&doc).unwrap().public
}

/// Two seats of size 1 centred at `min` and `max`, in rows of their own.
fn two_seats(min: [f32; 2], max: [f32; 2]) -> Vec<u8> {
    let block = |id: u32, label: &str, [x, y]: [f32; 2]| {
        format!(
            r#"{{"id":{id},"type":"rowBlock","origin":[{x},{y}],"category":"a","rows":[{{"rid":{},"seats":1,"label":"{label}","oids":[[{},1]]}}]}}"#,
            id + 1,
            id + 2
        )
    };
    compile(&format!(
        r##"{{"schema":1,"name":"t","settings":{{"seatSize":1}},"nextOid":10,
          "categories":[{{"key":"a","label":"A","color":"#ff0000"}}],
          "nodes":[{},{}]}}"##,
        block(1, "X", min),
        block(4, "Y", max)
    ))
}

/// An engine showing seats (size 1) centred at `min` and `max` in a
/// `width` × `height` view, after its first frame at time 0.
fn engine_with(min: [f32; 2], max: [f32; 2], width: f32, height: f32) -> E {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart {
        ccf: two_seats(min, max),
    });
    resize(&mut e, width, height, 1.0, 3.0);
    tick(&mut e, 0.0);
    e
}

fn engine() -> E {
    engine_with([0.0, 0.0], [999.0, 999.0], W, H)
}

/// A chart far larger than the view, entered in its middle, so neither the
/// zoom limits nor the clamp take part.
fn big() -> E {
    let mut e = engine_with([0.0, 0.0], [1e5, 1e5], W, H);
    set_camera(&mut e, [49_000.0, 49_000.0, 2000.0, 2000.0]);
    tick(&mut e, 0.0);
    e
}

fn resize(e: &mut E, width: f32, height: f32, dpr: f32, max_dpr: f32) {
    e.handle(Command::Resize {
        width,
        height,
        dpr,
        max_dpr,
    });
}

fn bounds([x, y, width, height]: [f32; 4]) -> CameraTarget {
    CameraTarget::Bounds(Bounds {
        x,
        y,
        width,
        height,
    })
}

fn set_camera(e: &mut E, b: [f32; 4]) {
    e.handle(Command::SetCamera { target: bounds(b) });
}

/// The events of one `Tick`, without those emitted before it.
fn tick(e: &mut E, time_ms: f64) -> Vec<Event> {
    e.drain_events().for_each(drop);
    e.handle(Command::Tick { time_ms });
    e.drain_events().collect()
}

fn ptr(e: &mut E, kind: PointerKind, id: u32, ty: PointerType, [x, y]: [f32; 2], t: f64) {
    let pointer_type = ty;
    e.handle(Command::Pointer {
        kind,
        id,
        pointer_type,
        x,
        y,
        time_ms: t,
    });
}

fn wheel(e: &mut E, [dx, dy]: [f32; 2], [x, y]: [f32; 2], ctrl: bool, time_ms: f64) {
    let modifiers = Modifiers {
        ctrl,
        ..Modifiers::default()
    };
    e.handle(Command::Wheel {
        dx,
        dy,
        x,
        y,
        modifiers,
        time_ms,
    });
}

/// A snapshot at `seq`: every object available but `unavailable`.
fn snapshot(e: &mut E, seq: u64, unavailable: &[u32]) -> Vec<Event> {
    let mut state = vec![0; e.objects().unwrap()];
    unavailable.iter().for_each(|&i| state[i as usize] = 1);
    e.drain_events().for_each(drop);
    e.handle(Command::SetSnapshot { seq, state });
    e.drain_events().collect()
}

/// A delta from `seq` to `seq + 1` of (`idx`, code) changes.
fn delta(e: &mut E, seq: u64, changes: &[(u32, u8)]) -> Vec<Event> {
    let changes = (changes.iter())
        .map(|&(idx, status)| crate::protocol::StatusChange { idx, status })
        .collect();
    e.drain_events().for_each(drop);
    e.handle(Command::ApplyDelta {
        from_seq: seq,
        to_seq: seq + 1,
        changes,
    });
    e.drain_events().collect()
}

fn cam(e: &E) -> Camera {
    e.camera.unwrap_or_else(|| panic!("no camera"))
}

fn wants_frame(events: &[Event]) -> bool {
    events.contains(&Event::NeedFrame)
}

/// Ticks every 16 ms from `from` until no frame is wanted; returns the time of
/// the last tick that still wanted one.
fn settle(e: &mut E, from: f64) -> f64 {
    let mut last = from;
    for t in (0..1000).map(|i| from + 16.0 * f64::from(i)) {
        if !wants_frame(&tick(e, t)) {
            return last;
        }
        last = t;
    }
    panic!("engine never settled");
}

fn close(a: [f64; 2], b: [f64; 2], tolerance: f64) -> bool {
    (a[0] - b[0]).abs() < tolerance && (a[1] - b[1]).abs() < tolerance
}

/// A small deterministic generator for property tests.
struct Rng(u64);

impl Rng {
    fn range(&mut self, lo: f64, hi: f64) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (lo + (hi - lo) * (self.0 >> 11) as f64 / (1u64 << 53) as f64) as f32
    }
}

#[test]
fn idle_engine_asks_for_and_draws_nothing() {
    let mut e = engine();
    assert_eq!(e.backend.draws.len(), 1);
    for t in 1..100 {
        assert!(tick(&mut e, f64::from(t) * 16.0).is_empty());
    }
    // A mouse hovering asks for a frame to apply its input, but draws nothing.
    ptr(&mut e, Move, 1, MOUSE, [10.0, 10.0], 2000.0);
    assert_eq!(e.drain_events().collect::<Vec<_>>(), [Event::NeedFrame]);
    assert!(tick(&mut e, 2000.0).is_empty());
    assert_eq!(e.backend.draws.len(), 1);
}

#[test]
fn zoom_and_centre_stay_within_limits() {
    // A chart so small that fitting it would exceed the maximum zoom.
    let e = engine_with([0.0, 0.0], [7.0, 7.0], W, H);
    assert_eq!(cam(&e).zoom, 64.0);
    let mut rng = Rng(0x5eed);
    for _ in 0..300 {
        // Charts store positions in quarter units.
        let q = |v: f32| (v * 4.0).round() / 4.0;
        let min = [q(rng.range(-1e4, 1e4)), q(rng.range(-1e4, 1e4))];
        let max = [
            q(min[0] + rng.range(0.0, 5e4)),
            q(min[1] + rng.range(0.0, 5e4)),
        ];
        let (w, h) = (rng.range(40.0, 3000.0), rng.range(40.0, 3000.0));
        let mut e = engine_with(min, max, w, h);
        let chart = [
            f64::from(max[0] - min[0] + 1.0),
            f64::from(max[1] - min[1] + 1.0),
        ];
        for step in 0..12 {
            let t = 1000.0 * f64::from(step + 1);
            let at = [rng.range(0.0, f64::from(w)), rng.range(0.0, f64::from(h))];
            match step % 4 {
                0 => wheel(&mut e, [0.0, rng.range(-3000.0, 3000.0)], at, true, t),
                1 => {
                    let to = [at[0] + rng.range(-5e3, 5e3), at[1] + rng.range(-5e3, 5e3)];
                    ptr(&mut e, Down, 1, MOUSE, at, t);
                    ptr(&mut e, Move, 1, MOUSE, to, t + 1.0);
                    ptr(&mut e, Up, 1, MOUSE, to, t + 500.0);
                }
                2 => set_camera(&mut e, [0, 0, 1, 1].map(|_| rng.range(1e-3, 1e5) - 5e4)),
                _ => resize(&mut e, rng.range(40.0, 3e3), rng.range(40.0, 3e3), 1.0, 3.0),
            }
            tick(&mut e, t + 999.0);
            let (c, size) = (
                cam(&e),
                e.stage().unwrap_or_else(|| panic!("no stage")).size,
            );
            let view = [size.width, size.height];
            // Spec limits: 0.8 × fit with 24 px padding, and a 1-unit seat at 64 px.
            let fit =
                ((view[0] - 48.0).max(1.0) / chart[0]).min((view[1] - 48.0).max(1.0) / chart[1]);
            let (lo, hi) = (0.8 * fit, 64f64.max(0.8 * fit));
            assert!(
                c.zoom >= lo * (1.0 - 1e-6) && c.zoom <= hi * (1.0 + 1e-6),
                "{} ∉ {lo}..{hi}",
                c.zoom
            );
            for axis in 0..2 {
                let edge = |v: f32| (f64::from(v) - c.centre[axis]) * c.zoom + view[axis] / 2.0;
                let (left, right) = (edge(min[axis] - 0.5), edge(max[axis] + 0.5));
                let shown = right.min(view[axis]) - left.max(0.0);
                let needed = (view[axis] / 3.0).min(right - left);
                assert!(
                    shown >= needed - 1e-6 * view[axis].max(right - left),
                    "{shown} < {needed}"
                );
            }
        }
    }
}

/// The last `CameraChanged` of `events`: its zoom and range.
fn reported_range(events: &[Event]) -> Option<(f32, [f32; 2])> {
    events.iter().rev().find_map(|event| match event {
        Event::CameraChanged { zoom, range, .. } => Some((*zoom, *range)),
        _ => None,
    })
}

#[test]
fn camera_changes_report_the_zoom_range_and_a_resize_reports_new_limits() {
    // A chart 1,000 units across in 800 × 600: fit is (600 − 48) / 1000.
    let mut e = engine();
    set_camera(&mut e, [0.0, 0.0, 1e-3, 1e-3]);
    let Some((zoom, [lo, hi])) = reported_range(&tick(&mut e, 1.0)) else {
        panic!("no CameraChanged");
    };
    assert_eq!(
        zoom, hi,
        "zoomed in past the limit, the camera is at the top"
    );
    assert_eq!(hi, 64.0);
    assert!((f64::from(lo) - 0.8 * 552.0 / 1000.0).abs() < 1e-6);
    // A narrower view lowers the fit, and with it the minimum.
    resize(&mut e, 400.0, 600.0, 1.0, 3.0);
    let Some((_, [narrow, _])) = reported_range(&tick(&mut e, 2.0)) else {
        panic!("a resize that changes the limits is reported");
    };
    assert!((f64::from(narrow) - 0.8 * 352.0 / 1000.0).abs() < 1e-6);
    assert_eq!(reported_range(&tick(&mut e, 3.0)), None, "nothing changed");
}

#[test]
fn wheel_zoom_keeps_the_point_under_the_cursor() {
    let mut rng = Rng(7);
    let mut e = big();
    for i in 0..200 {
        let at = [rng.range(0.0, f64::from(W)), rng.range(0.0, f64::from(H))];
        let before = cam(&e).to_world(at.map(f64::from), SIZE);
        // Mouse-wheel notches and trackpad pinches, in and out.
        let t = 1000.0 * f64::from(i + 1);
        let notch = if rng.range(0.0, 1.0) < 0.5 {
            100.0
        } else {
            -100.0
        };
        let (dy, ctrl) = if i % 2 == 0 {
            (notch, false)
        } else {
            (rng.range(-30.0, 30.0), true)
        };
        wheel(&mut e, [0.0, dy], at, ctrl, t);
        tick(&mut e, t);
        assert!(close(
            cam(&e).to_world(at.map(f64::from), SIZE),
            before,
            1e-6
        ));
    }
}

#[test]
fn fly_to_lands_exactly_within_its_duration() {
    let target = [100.0, 200.0, 50.0, 40.0];
    let mut reference = engine();
    set_camera(&mut reference, target);
    tick(&mut reference, 1.0);
    let expected = cam(&reference);

    for (duration_ms, lo, hi) in [(None, 250.0, 600.0), (Some(400), 400.0, 400.0)] {
        let mut e = engine();
        e.handle(Command::FlyTo {
            target: bounds(target),
            duration_ms,
        });
        tick(&mut e, 1000.0);
        // It lands on the first 16 ms tick at or after its duration.
        let took = settle(&mut e, 1016.0) + 16.0 - 1000.0;
        assert!(took >= lo && took < hi + 16.0, "took {took} ms");
        assert_eq!(cam(&e), expected);
    }

    let mut e = engine();
    e.handle(Command::SetReducedMotion(true));
    e.handle(Command::FlyTo {
        target: bounds(target),
        duration_ms: None,
    });
    assert!(!wants_frame(&tick(&mut e, 1000.0)));
    assert_eq!(cam(&e), expected);

    // Reduced motion requested mid-flight lands it at the next frame.
    let mut e = engine();
    let duration_ms = Some(400);
    e.handle(Command::FlyTo {
        target: bounds(target),
        duration_ms,
    });
    tick(&mut e, 1000.0);
    tick(&mut e, 1016.0);
    e.handle(Command::SetReducedMotion(true));
    assert!(!wants_frame(&tick(&mut e, 1032.0)));
    assert_eq!(cam(&e), expected);

    // A resize mid-flight that raises the minimum zoom holds at landing.
    let mut e = engine();
    let target = bounds([0.0, 0.0, 2000.0, 2000.0]);
    e.handle(Command::FlyTo {
        target,
        duration_ms,
    });
    tick(&mut e, 1000.0);
    resize(&mut e, 1600.0, 1200.0, 1.0, 3.0);
    settle(&mut e, 1016.0);
    let min_zoom = 0.8 * (1200.0 - 48.0) / 1000.0;
    assert!(
        (cam(&e).zoom - min_zoom).abs() < 1e-9,
        "zoom {}",
        cam(&e).zoom
    );
}

#[test]
fn a_press_becomes_a_drag_past_the_threshold() {
    for (ty, under, over) in [(MOUSE, 3.5, 4.5), (TOUCH, 7.5, 8.5)] {
        let mut e = engine();
        let start = cam(&e);
        ptr(&mut e, Down, 1, ty, [400.0, 300.0], 10.0);
        ptr(&mut e, Move, 1, ty, [400.0 + under, 300.0], 20.0);
        tick(&mut e, 20.0);
        assert_eq!(cam(&e), start, "{ty:?} moved under the threshold");
        ptr(&mut e, Move, 1, ty, [400.0 + over, 300.0], 30.0);
        tick(&mut e, 30.0);
        // The chart is grabbed where the press began.
        let moved = (start.centre[0] - cam(&e).centre[0]) * start.zoom;
        assert!(
            (moved - f64::from(over)).abs() < 1e-9,
            "{ty:?} moved {moved}"
        );
        ptr(&mut e, Up, 1, ty, [400.0 + over, 300.0], 2000.0);
        tick(&mut e, 2000.0);
        assert!(e.intents.is_empty(), "a drag is not a tap");
    }
}

#[test]
fn pinch_zooms_about_a_fixed_midpoint() {
    let mut e = engine();
    let mid = [400.0, 300.0];
    let (anchor, zoom) = (cam(&e).to_world(mid, SIZE), cam(&e).zoom);
    ptr(&mut e, Down, 1, TOUCH, [350.0, 300.0], 0.0);
    ptr(&mut e, Down, 2, TOUCH, [450.0, 300.0], 5.0);
    for (i, d) in [60.0, 75.0, 90.0, 100.0].into_iter().enumerate() {
        let t = 20.0 * (i as f64 + 1.0);
        ptr(&mut e, Move, 1, TOUCH, [400.0 - d, 300.0], t);
        ptr(&mut e, Move, 2, TOUCH, [400.0 + d, 300.0], t + 1.0);
        tick(&mut e, t + 2.0);
    }
    assert!(close(cam(&e).to_world(mid, SIZE), anchor, 1e-9));
    assert!((cam(&e).zoom / zoom - 2.0).abs() < 1e-9);

    // Movement under the drag threshold is not lost when the pinch begins.
    let mut e = engine();
    ptr(&mut e, Down, 1, TOUCH, [350.0, 300.0], 0.0);
    ptr(&mut e, Down, 2, TOUCH, [450.0, 300.0], 5.0);
    for (i, d) in [57.0, 57.0, 59.0, 59.0].into_iter().enumerate() {
        ptr(
            &mut e,
            Move,
            1 + i as u32 % 2,
            TOUCH,
            [400.0 + d * if i % 2 == 0 { -1.0 } else { 1.0 }, 300.0],
            10.0 + i as f64,
        );
    }
    tick(&mut e, 20.0);
    assert!(
        (cam(&e).zoom / zoom - 1.18).abs() < 1e-9,
        "zoom ×{}",
        cam(&e).zoom / zoom
    );
}

#[test]
fn inertia_decays_stops_and_then_asks_for_nothing() {
    let mut e = big();
    // Drag left at 1 px/ms, then release while moving.
    ptr(&mut e, Down, 1, TOUCH, [600.0, 300.0], 0.0);
    for t in (1..=10).map(|i| f64::from(i) * 10.0) {
        ptr(&mut e, Move, 1, TOUCH, [600.0 - t as f32, 300.0], t);
    }
    ptr(&mut e, Up, 1, TOUCH, [490.0, 300.0], 110.0);
    tick(&mut e, 110.0);
    let released = cam(&e);
    let (mut steps, mut prev, mut t) = (Vec::new(), released.centre[0], 126.0);
    while wants_frame(&tick(&mut e, t)) {
        steps.push(cam(&e).centre[0] - prev);
        prev = cam(&e).centre[0];
        t += 16.0;
    }
    assert!(
        steps.windows(2).all(|w| w[1] <= w[0] + 1e-12),
        "inertia must decay"
    );
    // 1 px/ms stops at 10 px/s after tau·ln(100) ≈ 1497 ms, having travelled
    // v·tau·(1 − 0.01) ≈ 321.75 px.
    let travelled = (cam(&e).centre[0] - released.centre[0]) * released.zoom;
    assert!((travelled - 321.75).abs() < 0.5, "travelled {travelled}");
    assert!(t - 110.0 < 1600.0);
    assert_eq!(e.backend.draws.len(), steps.len() + 4);
    for later in 0..10 {
        assert!(tick(&mut e, t + f64::from(later) * 16.0).is_empty());
    }

    // A flick that crosses the threshold in one move still flings, also by
    // the finger left after a two-finger press.
    let mut e = big();
    ptr(&mut e, Down, 1, TOUCH, [600.0, 300.0], 0.0);
    ptr(&mut e, Move, 1, TOUCH, [500.0, 300.0], 50.0);
    ptr(&mut e, Up, 1, TOUCH, [500.0, 300.0], 51.0);
    assert!(wants_frame(&tick(&mut e, 51.0)));
    let mut e = big();
    ptr(&mut e, Down, 1, TOUCH, [200.0, 300.0], 0.0);
    ptr(&mut e, Down, 2, TOUCH, [600.0, 300.0], 180.0);
    ptr(&mut e, Up, 1, TOUCH, [200.0, 300.0], 185.0);
    ptr(&mut e, Move, 2, TOUCH, [500.0, 300.0], 190.0);
    ptr(&mut e, Up, 2, TOUCH, [500.0, 300.0], 191.0);
    assert!(wants_frame(&tick(&mut e, 191.0)));

    // No inertia under reduced motion.
    let mut e = engine();
    e.handle(Command::SetReducedMotion(true));
    ptr(&mut e, Down, 1, MOUSE, [600.0, 300.0], 0.0);
    ptr(&mut e, Move, 1, MOUSE, [500.0, 300.0], 50.0);
    ptr(&mut e, Up, 1, MOUSE, [490.0, 300.0], 60.0);
    assert!(!wants_frame(&tick(&mut e, 60.0)));
}

#[test]
fn double_tap_zooms_in_and_two_finger_tap_zooms_out() {
    let mut e = engine();
    let zoom = cam(&e).zoom;
    for (id, at, t) in [(1, [300.0, 200.0], 0.0), (2, [305.0, 203.0], 200.0)] {
        ptr(&mut e, Down, id, TOUCH, at, t);
        ptr(&mut e, Up, id, TOUCH, at, t + 80.0);
    }
    settle(&mut e, 300.0);
    assert!((cam(&e).zoom / zoom - 2.0).abs() < 1e-9);
    // Only the first tap of the pair reaches hit testing.
    assert_eq!(e.intents.len(), 1);

    ptr(&mut e, Down, 3, TOUCH, [300.0, 300.0], 5000.0);
    ptr(&mut e, Down, 4, TOUCH, [500.0, 300.0], 5020.0);
    ptr(&mut e, Up, 3, TOUCH, [301.0, 300.0], 5100.0);
    ptr(&mut e, Up, 4, TOUCH, [500.0, 301.0], 5110.0);
    settle(&mut e, 5120.0);
    assert!((cam(&e).zoom / zoom - 1.0).abs() < 1e-9);
    assert_eq!(e.intents.len(), 1, "a two-finger tap is not a tap");

    // The threshold counts from each finger's press, even when the first
    // moved before the second went down: 14 px is no tap.
    let mut e = engine();
    ptr(&mut e, Down, 1, TOUCH, [350.0, 300.0], 0.0);
    ptr(&mut e, Move, 1, TOUCH, [357.0, 300.0], 10.0);
    ptr(&mut e, Down, 2, TOUCH, [450.0, 300.0], 20.0);
    ptr(&mut e, Move, 1, TOUCH, [364.0, 300.0], 30.0);
    ptr(&mut e, Up, 1, TOUCH, [364.0, 300.0], 100.0);
    ptr(&mut e, Up, 2, TOUCH, [450.0, 300.0], 110.0);
    assert!(!wants_frame(&tick(&mut e, 110.0)), "no zoom-out animation");
}

#[test]
fn taps_and_long_presses_are_recognised() {
    let mut e = engine();
    // A quick touch is a tap; a still touch held past 500 ms is a long press,
    // seen on the tick that crosses it, and gives no tap on release.
    ptr(&mut e, Down, 1, TOUCH, [100.0, 100.0], 0.0);
    ptr(&mut e, Up, 1, TOUCH, [102.0, 101.0], 120.0);
    tick(&mut e, 130.0);
    ptr(&mut e, Down, 2, TOUCH, [200.0, 100.0], 1000.0);
    assert!(wants_frame(&tick(&mut e, 1016.0)));
    assert!(wants_frame(&tick(&mut e, 1400.0)));
    assert!(!wants_frame(&tick(&mut e, 1510.0)));
    ptr(&mut e, Up, 2, TOUCH, [200.0, 100.0], 2000.0);
    // A mouse held still for a second is still a click.
    ptr(&mut e, Down, 3, MOUSE, [300.0, 100.0], 3000.0);
    ptr(&mut e, Up, 3, MOUSE, [300.0, 100.0], 4000.0);
    tick(&mut e, 4000.0);
    let expected = [
        Intent::Tap {
            at: [102.0, 101.0],
            pointer: TOUCH,
        },
        Intent::LongPress { at: [200.0, 100.0] },
        Intent::Tap {
            at: [300.0, 100.0],
            pointer: MOUSE,
        },
    ];
    assert_eq!(e.intents, expected);
    // A late tick does not turn a long hold into a tap.
    let mut e = engine();
    ptr(&mut e, Down, 1, TOUCH, [100.0, 100.0], 0.0);
    ptr(&mut e, Up, 1, TOUCH, [100.0, 100.0], 700.0);
    tick(&mut e, 700.0);
    assert_eq!(e.intents, [Intent::LongPress { at: [100.0, 100.0] }]);
}

#[test]
fn cancel_mid_gesture_leaves_a_clean_state() {
    let mut e = engine();
    ptr(&mut e, Down, 1, TOUCH, [400.0, 300.0], 0.0);
    ptr(&mut e, Move, 1, TOUCH, [300.0, 300.0], 50.0);
    ptr(&mut e, Cancel, 1, TOUCH, [300.0, 300.0], 60.0);
    assert!(
        !wants_frame(&tick(&mut e, 60.0)),
        "no inertia after a cancel"
    );
    // A pinch losing one finger: the other no longer moves the map.
    ptr(&mut e, Down, 2, TOUCH, [300.0, 300.0], 100.0);
    ptr(&mut e, Down, 3, TOUCH, [500.0, 300.0], 110.0);
    ptr(&mut e, Cancel, 2, TOUCH, [300.0, 300.0], 120.0);
    tick(&mut e, 120.0);
    let still = cam(&e);
    ptr(&mut e, Move, 3, TOUCH, [600.0, 400.0], 130.0);
    ptr(&mut e, Up, 3, TOUCH, [600.0, 400.0], 140.0);
    assert!(!wants_frame(&tick(&mut e, 140.0)));
    assert_eq!(cam(&e), still);
    assert!(e.intents.is_empty());
    // The next tap is recognised normally, and nothing keeps asking for frames.
    ptr(&mut e, Down, 4, TOUCH, [100.0, 100.0], 1000.0);
    ptr(&mut e, Up, 4, TOUCH, [100.0, 100.0], 1050.0);
    assert!(!wants_frame(&tick(&mut e, 1050.0)));
    assert_eq!(e.intents.len(), 1);
}

#[test]
fn cooperative_mode_leaves_one_finger_and_plain_wheel_to_the_page() {
    let mut e = engine();
    e.handle(Command::SetGestureHandling(GestureHandling::Cooperative));
    let start = cam(&e);
    ptr(&mut e, Down, 1, TOUCH, [400.0, 300.0], 0.0);
    ptr(&mut e, Move, 1, TOUCH, [400.0, 100.0], 50.0);
    ptr(&mut e, Up, 1, TOUCH, [400.0, 100.0], 60.0);
    tick(&mut e, 60.0);
    wheel(&mut e, [0.0, 100.0], [400.0, 300.0], false, 100.0);
    assert!(
        e.drain_events().next().is_none(),
        "a plain wheel is not even queued"
    );
    assert_eq!(cam(&e), start);
    // Ctrl+wheel, two fingers and mouse drags move the map.
    wheel(&mut e, [0.0, -100.0], [400.0, 300.0], true, 200.0);
    tick(&mut e, 200.0);
    assert!(cam(&e).zoom > start.zoom);
    let zoomed = cam(&e);
    ptr(&mut e, Down, 2, TOUCH, [300.0, 300.0], 1000.0);
    ptr(&mut e, Down, 3, TOUCH, [500.0, 300.0], 1001.0);
    ptr(&mut e, Move, 3, TOUCH, [520.0, 300.0], 1020.0);
    ptr(&mut e, Up, 2, TOUCH, [300.0, 300.0], 1030.0);
    ptr(&mut e, Up, 3, TOUCH, [520.0, 300.0], 1030.0);
    tick(&mut e, 1030.0);
    assert!(cam(&e).zoom > zoomed.zoom);
    let pinched = cam(&e);
    ptr(&mut e, Down, 4, MOUSE, [400.0, 300.0], 2000.0);
    ptr(&mut e, Move, 4, MOUSE, [450.0, 300.0], 2010.0);
    tick(&mut e, 2010.0);
    assert_ne!(cam(&e), pinched);

    // `none`: nothing moves the map, but taps still reach hit testing.
    let mut e = engine();
    e.handle(Command::SetGestureHandling(GestureHandling::None));
    let start = cam(&e);
    wheel(&mut e, [0.0, -100.0], [400.0, 300.0], true, 0.0);
    ptr(&mut e, Down, 1, MOUSE, [400.0, 300.0], 10.0);
    ptr(&mut e, Move, 1, MOUSE, [100.0, 300.0], 20.0);
    ptr(&mut e, Up, 1, MOUSE, [100.0, 300.0], 30.0);
    for (id, t) in [(2, 100.0), (3, 250.0)] {
        ptr(&mut e, Down, id, TOUCH, [200.0, 200.0], t);
        ptr(&mut e, Up, id, TOUCH, [200.0, 200.0], t + 50.0);
    }
    settle(&mut e, 300.0);
    assert_eq!(cam(&e), start);
    assert_eq!(
        e.intents.len(),
        2,
        "both taps of a double tap are taps when it cannot zoom"
    );
}

#[test]
fn resizes_coalesce_and_the_backing_store_is_capped() {
    let mut e = engine();
    for (width, height) in [(100.0, 100.0), (300.0, 200.0), (1000.0, 500.0)] {
        resize(&mut e, width, height, 3.0, 2.0);
    }
    tick(&mut e, 16.0);
    // One frame at the last size, at `maxDpr` 2.
    assert_eq!(
        e.backend.targets[1..],
        [Target {
            width: 2000,
            height: 1000
        }]
    );
    // The long side is capped at 4,096 device pixels.
    resize(&mut e, 3000.0, 1000.0, 3.0, 3.0);
    tick(&mut e, 32.0);
    assert_eq!(
        e.backend.targets.last(),
        Some(&Target {
            width: 4096,
            height: 1365
        })
    );
}

#[test]
fn a_recorded_trace_replays_identically() {
    let trace = |e: &mut E| {
        let mut events = Vec::new();
        let mut frames = |e: &mut E, n: u32, t: &mut f64| {
            for _ in 0..n {
                *t += 16.0;
                events.extend(tick(e, *t));
            }
        };
        let mut t = 0.0;
        ptr(e, Down, 1, TOUCH, [500.0, 300.0], 1.0);
        for i in 0..8 {
            let at = [500.0 - 13.0 * i as f32, 300.0 + 3.0 * i as f32];
            ptr(e, Move, 1, TOUCH, at, 2.0 + 12.0 * f64::from(i));
            frames(e, 1, &mut t);
        }
        ptr(e, Up, 1, TOUCH, [400.0, 324.0], 100.0);
        frames(e, 20, &mut t);
        wheel(e, [0.0, -120.0], [200.0, 200.0], false, t);
        let target = bounds([10.0, 10.0, 100.0, 100.0]);
        e.handle(Command::FlyTo {
            target,
            duration_ms: None,
        });
        frames(e, 50, &mut t);
        events
    };
    let first = trace(&mut engine());
    assert!(
        first
            .iter()
            .filter(|e| matches!(e, Event::CameraChanged { .. }))
            .count()
            > 20
    );
    assert_eq!(first, trace(&mut engine()));
}

/// The loaded chart's GPU resources.
fn gpu(e: &E) -> &Gpu {
    e.scene.as_ref().unwrap().gpu.as_ref().unwrap()
}
