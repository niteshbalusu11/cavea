//! A lost and restored context (04 §20.2) on the recording backend: no GPU
//! call while lost, every resource made again exactly once from what the
//! engine keeps, the same picture afterwards, and every order of loss,
//! restore, load and drop safe.

use super::{
    labels::{answer, centre_of, chart, look, rasterise, requests},
    *,
};
use crate::protocol::{BuiltinTheme, ObjectRef};

/// Everything the backend would put on screen: the contents of every live
/// buffer and texture, sorted (ids differ after a restore), and the last
/// frame's draws.
#[allow(clippy::type_complexity)]
fn picture(
    e: &E,
    from: usize,
) -> (
    Vec<Vec<u8>>,
    Vec<Vec<u8>>,
    Vec<(Pipeline, Vec<Range<u32>>, f32)>,
) {
    let mut buffers: Vec<_> = e.backend.buffers.values().cloned().collect();
    let mut textures: Vec<_> = e.backend.textures.values().map(|t| t.1.clone()).collect();
    buffers.sort();
    textures.sort();
    (buffers, textures, e.backend.calls[from..].to_vec())
}

/// GPU calls so far other than `destroy`: creations, writes, frames, draws.
fn gpu_calls(e: &E) -> usize {
    let b = &e.backend;
    b.next as usize
        + b.buffer_writes.len()
        + b.texture_writes.len()
        + b.targets.len()
        + b.calls.len()
}

/// The labelled chart at the detail level over South, in the dark theme,
/// with seat 43 unavailable and seat 45 selected: seat labels, row labels
/// from both atlases (row `Ä`), icons and the dynamic atlas all in use. The
/// view is the first drawn, so the dynamic atlas holds only what it shows,
/// as it will after a restore.
fn busy() -> E {
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf: chart("both") });
    e.handle(super::text::atlas());
    resize(&mut e, W, H, 1.0, 3.0);
    let centre = centre_of(&e, 1);
    look(&mut e, centre, 20.0);
    snapshot(&mut e, 1, &[43]);
    e.handle(Command::SetTheme {
        base: BuiltinTheme::Dark,
        tokens: Vec::new(),
    });
    e.handle(Command::Select {
        objects: vec![ObjectRef::Idx(45)],
    });
    rasterise(&mut e, 100.0);
    e
}

#[test]
fn a_restored_context_gets_every_resource_once_and_the_same_picture() {
    let mut e = busy();
    let from = e.backend.calls.len();
    e.dirty = true;
    tick(&mut e, 200.0);
    let before = picture(&e, from);
    // Seat states, selection included, and the camera.
    let state = |e: &E| (e.camera, e.scene.as_ref().unwrap().scene.visual.clone());
    let kept = state(&e);
    let live = e.backend.buffers.len() + e.backend.textures.len();

    e.handle(Command::ContextLost);
    // Everything is let go at once, and nothing touches the GPU while lost,
    // whatever arrives meanwhile.
    assert!(e.backend.buffers.is_empty() && e.backend.textures.is_empty());
    let calls = gpu_calls(&e);
    delta(&mut e, 1, &[(7, 1)]);
    delta(&mut e, 2, &[(7, 0)]);
    e.handle(Command::SetTheme {
        base: BuiltinTheme::Dark,
        tokens: Vec::new(),
    });
    e.handle(super::text::atlas());
    for t in [300.0, 316.0, 332.0] {
        assert!(requests(&tick(&mut e, t)).is_empty());
    }
    assert_eq!(gpu_calls(&e), calls);

    e.handle(Command::ContextRestored);
    assert_eq!(e.backend.restores, 1);
    let made = e.backend.next;
    // The strings are asked for again; once answered, the picture is back.
    let events = tick(&mut e, 400.0);
    let asked = requests(&events);
    assert!(asked.iter().any(|a| a.1 == "Ä"), "{events:?}");
    answer(&mut e, &asked);
    rasterise(&mut e, 420.0);
    // (The deltas made while lost pulse seat 7 for a while.)
    settle(&mut e, 500.0);
    let from = e.backend.calls.len();
    e.dirty = true;
    tick(&mut e, 5000.0);
    let after = picture(&e, from);
    for (i, (a, b)) in after.0.iter().zip(&before.0).enumerate() {
        let at = a.iter().zip(b).position(|(x, y)| x != y);
        assert!(
            a == b,
            "buffer {i}: len {} vs {}, first diff at {at:?}",
            a.len(),
            b.len()
        );
    }
    assert_eq!(after.0.len(), before.0.len());
    for (i, (a, b)) in after.1.iter().zip(&before.1).enumerate() {
        let at = a.iter().zip(b).position(|(x, y)| x != y);
        assert!(
            a == b,
            "texture {i}: len {} vs {}, first diff at {at:?}",
            a.len(),
            b.len()
        );
    }
    assert_eq!(after.2, before.2);
    assert_eq!(state(&e), kept);
    // Each resource was made once: as many as were alive, and no more on
    // later frames.
    assert_eq!((e.backend.next - made) as usize, live);
    e.dirty = true;
    tick(&mut e, 5100.0);
    assert_eq!((e.backend.next - made) as usize, live);
}

#[test]
fn losses_restores_loads_and_drops_are_safe_in_any_order() {
    // Lost before the chart and the atlas arrive: they load, nothing is
    // made or drawn until the context returns.
    let mut e = Engine::new(Fake::default());
    e.handle(Command::ContextLost);
    e.handle(Command::LoadChart {
        ccf: two_seats([0.0, 0.0], [10.0, 10.0]),
    });
    let ready = e.drain_events().any(|e| matches!(e, Event::Ready(_)));
    assert!(ready);
    e.handle(super::text::atlas());
    resize(&mut e, W, H, 1.0, 3.0);
    tick(&mut e, 0.0);
    assert_eq!(gpu_calls(&e), 0);
    // Lost twice, restored twice: one restore.
    e.handle(Command::ContextLost);
    e.handle(Command::ContextRestored);
    e.handle(Command::ContextRestored);
    assert_eq!(e.backend.restores, 1);
    tick(&mut e, 16.0);
    assert!(!e.backend.calls.is_empty(), "drawn once restored");

    // A restore that fails leaves the engine lost and says so; the next one
    // works.
    e.handle(Command::ContextLost);
    e.backend.fail_restore = true;
    e.handle(Command::ContextRestored);
    let errors = e
        .drain_events()
        .filter(|e| matches!(e, Event::Error { .. }))
        .count();
    assert_eq!(errors, 1);
    let calls = gpu_calls(&e);
    e.dirty = true;
    tick(&mut e, 32.0);
    assert_eq!(gpu_calls(&e), calls);
    e.backend.fail_restore = false;
    e.handle(Command::ContextRestored);
    tick(&mut e, 48.0);
    assert!(gpu_calls(&e) > calls);

    // Dropped while lost: the engine holds no resource, so dropping it
    // makes no GPU call.
    e.handle(Command::ContextLost);
    let loaded = e.scene.as_ref().unwrap();
    assert!(loaded.gpu.is_none() && loaded.seat_labels.buffer().is_none());
    assert!(loaded.map_labels.text_buffer().is_none() && loaded.text_atlas.texture().is_none());
    assert!(e.atlas.as_ref().is_some_and(|a| a.texture.is_none()));
    drop(e);
}

/// Native timing for 04 §20.2: the restore and the first frame after it on
/// `stadium-100k` at the detail level, CPU only (the test backend copies
/// the bytes). Run with `cargo test --release -p cavea-render
/// measure_restore -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a test"]
fn measure_restore() {
    let preset = cavea_testkit::presets::find("stadium-100k").unwrap();
    let ccf = cavea_compiler::compile(&preset.generate(1)).unwrap().public;
    let mut e = Engine::new(Fake::default());
    e.handle(Command::LoadChart { ccf });
    e.handle(super::text::atlas());
    resize(&mut e, 1920.0, 1080.0, 1.0, 3.0);
    tick(&mut e, 0.0);
    let [x, y] = e.scene.as_ref().unwrap().scene.chart.sections[0].label_anchor;
    look(&mut e, [f64::from(x) / 4.0, f64::from(y) / 4.0], 20.0);
    rasterise(&mut e, 16.0);
    let runs = 50u32;
    let start = std::time::Instant::now();
    for i in 0..runs {
        e.handle(Command::ContextLost);
        e.handle(Command::ContextRestored);
        tick(&mut e, 100.0 + f64::from(i));
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
    println!("stadium-100k at detail: restore and first frame in {ms:.2} ms (CPU, test backend)");
}
