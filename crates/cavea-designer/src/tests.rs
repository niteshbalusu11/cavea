//! The canvas against the buyer's renderer and against itself (04 §25,
//! 07 §14): the live scene of a preset's draft equals the scene the buyer
//! loads from its compiled chart, order aside; after random edits the scene
//! kept up to date equals one built from scratch, and the GPU holds exactly
//! its bytes; pieces moved in a gesture are drawn where baking puts them;
//! a lost context comes back without compiling anything.

use cavea_compiler::ChartDocument;
use cavea_compiler::document::{LabelStart, SeatLabels};
use cavea_edit::random::{Rng, command};
use cavea_edit::{Button, Input, Level, Modifiers, Pointer, PointerKind, Session, ToolId};
use cavea_format::{Chart, full_label::seat_label};
use cavea_render::{backend::record::Recorder, protocol::Command, scene::Scene};

use crate::paint::{Painter, View};

fn preset(name: &str) -> ChartDocument {
    cavea_testkit::presets::find(name)
        .expect("preset")
        .generate(cavea_testkit::presets::DEFAULT_SEED)
}

fn painter(view: View) -> Painter<Recorder> {
    let mut p = Painter::new(Recorder::default());
    p.set_theme(false);
    assert!(p.set_view(view));
    p
}

const VIEW: View = View {
    centre: [0.0, 0.0],
    zoom: 0.05,
    width: 1000.0,
    height: 800.0,
    dpr: 1.0,
};

/// Draws until the scene has nothing left to bake (and at most a few
/// frames for labels the engine builds over frames).
fn settle(p: &mut Painter<Recorder>, s: &Session) {
    for t in 0..64 {
        let again = p.draw(s, f64::from(t) * 16.0);
        if !again && !p.sync().pending() {
            break;
        }
    }
}

/// Everything a scene draws on its floor, resolved to values and sorted,
/// so two scenes laid out in different orders compare equal.
#[derive(Debug, PartialEq)]
struct Drawn {
    seat_size: u16,
    fills: bool,
    seats: Vec<String>,
    rows: Vec<String>,
    sections: Vec<String>,
    texts: Vec<String>,
    icons: Vec<String>,
    decoration: Vec<String>,
}

fn drawn(scene: &Scene) -> Drawn {
    let c: &Chart = &scene.chart;
    let s = |i: u32| c.strings.get(i).unwrap_or_default().to_owned();
    let floor = scene.floor;
    let cat = |i: u32| {
        c.categories
            .get(i as usize)
            .map_or(String::new(), |k| s(k.key))
    };
    let section_label = |i: u32| {
        c.sections
            .get(i as usize)
            .map_or(String::new(), |x| s(x.label))
    };
    let mut seats = Vec::new();
    for idx in scene.active.clone() {
        let i = idx as usize;
        let row = &c.rows[scene.row[i] as usize];
        let label = seat_label(c, idx).expect("label");
        let p = scene.pos[i];
        seats.push(format!(
            "{:?} {} {:?} {} {} {}|{}|{} {}",
            [(p[0] * 4.0) as i64, (p[1] * 4.0) as i64],
            row.seat_size,
            row.shape,
            cat(u32::from(c.seat_category[i])),
            c.seat_flags[i],
            label.section,
            label.row,
            label.seat.as_str(),
            section_label(row.section),
        ));
    }
    let rows = (c.rows.iter())
        .filter(|r| scene.active.contains(&r.seat_first))
        .map(|r| {
            format!(
                "{} {:?} {:?} {:?} {} {} {:?} {} {}",
                s(r.label),
                r.label_start,
                r.label_end,
                r.bounds,
                r.seat_count,
                r.positions,
                r.direction,
                r.ordinal_base,
                section_label(r.section),
            )
        })
        .collect();
    let triangles = |range: std::ops::Range<u32>, colour: bool| {
        let idx = &c.mesh.indices[range.start as usize..range.end as usize];
        let mut tris: Vec<String> = idx
            .chunks(3)
            .map(|t| {
                let mut v: Vec<String> = t
                    .iter()
                    .map(|&i| {
                        let v = &c.mesh.vertices[i as usize];
                        let colour = if colour { v.color } else { [0; 4] };
                        format!("{:?}{:?}", v.pos.map(f32::to_bits), colour)
                    })
                    .collect();
                v.sort();
                v.join(" ")
            })
            .collect();
        tris.sort();
        tris
    };
    let sections = (c.sections.iter())
        .filter(|x| x.floor == floor)
        .map(|x| {
            format!(
                "{} {:?} {} {:?} {} {} {} {} {:?}",
                s(x.label),
                x.label_anchor,
                x.label_size,
                x.bounds,
                x.seat_count,
                x.row_count,
                cat(x.category),
                s(x.entrance),
                triangles(x.mesh_first..x.mesh_first + x.mesh_count, false),
            )
        })
        .collect();
    let texts = (c.texts.iter())
        .filter(|t| t.floor == floor)
        .map(|t| {
            format!(
                "{} {:?} {} {} {:?} {:?} {} {}",
                s(t.text),
                t.pos,
                t.size,
                t.angle,
                t.color,
                t.align,
                t.weight,
                t.min_zoom
            )
        })
        .collect();
    let icons = (c.icons.iter())
        .filter(|i| i.floor == floor)
        .map(|i| format!("{} {:?} {} {}", s(i.key), i.pos, i.size, i.angle))
        .collect();
    let mut decoration: Vec<String> = (c.mesh.draws.iter())
        .filter(|d| d.floor == floor)
        .flat_map(|d| {
            triangles(d.index_first..d.index_first + d.index_count, true)
                .into_iter()
                .map(move |t| format!("{:?} {t}", d.layer))
        })
        .collect();
    decoration.sort();
    let mut d = Drawn {
        seat_size: c.meta.seat_size,
        fills: c
            .sections
            .iter()
            .any(|x| x.floor == floor && x.mesh_count > 0),
        seats,
        rows,
        sections,
        texts,
        icons,
        decoration,
    };
    d.seats.sort();
    d.rows.sort();
    d.sections.sort();
    d.texts.sort();
    d.icons.sort();
    d
}

/// The buyer's scene of the compiled chart, showing floor `floor`.
fn buyer(doc: &ChartDocument, floor: usize) -> Scene {
    let compiled = cavea_compiler::compile(doc).expect("compiles");
    let mut scene = Scene::decode(&compiled.public).expect("decodes");
    let f = &scene.chart.floors[floor];
    scene.active = f.seat_first..f.seat_first + f.seat_count;
    scene.floor = floor as u32;
    scene
}

/// 04 §25: for the presets, the designer draws what the buyer draws, on
/// every floor: the same seats (position, size, shape, category, flags,
/// labels), rows, sections (label, anchor, size, box, fill), text objects,
/// icons and decoration, and the same seat size and fills, so the same
/// levels of detail.
#[test]
fn presets_match_the_buyer() {
    for name in ["theatre", "arena", "stadium-60k"] {
        let doc = preset(name);
        let mut s = Session::load(doc.clone()).expect("load");
        let floors: Vec<u32> = s.document().floors().iter().map(|f| f.id).collect();
        for (i, &f) in floors.iter().enumerate().take(2) {
            assert!(s.set_floor(f));
            let mut p = painter(VIEW);
            settle(&mut p, &s);
            let live = p.engine().scene().expect("scene");
            let want = buyer(&doc, i);
            let (a, b) = (drawn(live), drawn(&want));
            assert!(!b.seats.is_empty() || name == "theatre", "{name}");
            assert_eq!(a.seat_size, b.seat_size, "{name} floor {f}");
            assert_eq!(a.fills, b.fills, "{name} floor {f}");
            assert_eq!(a.seats, b.seats, "{name} floor {f}: seats");
            assert_eq!(a.rows, b.rows, "{name} floor {f}: rows");
            assert_eq!(a.sections, b.sections, "{name} floor {f}: sections");
            assert_eq!(a.texts, b.texts, "{name} floor {f}: texts");
            assert_eq!(a.icons, b.icons, "{name} floor {f}: icons");
            assert_eq!(a.decoration, b.decoration, "{name} floor {f}: decoration");
        }
    }
}

/// What the GPU holds equals the scene's own bytes: the seats' static and
/// dynamic buffers and the mesh.
fn gpu_matches(p: &Painter<Recorder>, what: &str) {
    let engine = p.engine();
    let scene = engine.scene().expect("scene");
    let ([st, dy], mesh) = engine.live_buffers().expect("buffers");
    let rec = engine.backend();
    let n = scene.pos.len();
    let statics = scene.encode_static();
    assert_eq!(
        &rec.buffer(st).expect("static")[..statics.len()],
        &statics[..],
        "{what}: static seat bytes"
    );
    let c = &scene.chart;
    let dynamics =
        cavea_render::seat::encode_dynamic(&c.seat_category, &scene.visual, &scene.fx, 0..n);
    assert_eq!(
        &rec.buffer(dy).expect("dynamic")[..dynamics.len()],
        &dynamics[..],
        "{what}: dynamic seat bytes"
    );
    if let Some([v, i]) = mesh {
        let indices: Vec<u8> = c
            .mesh
            .indices
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        assert_eq!(
            rec.buffer(i).expect("indices"),
            &indices[..],
            "{what}: indices"
        );
        let vertices: Vec<u8> = c
            .mesh
            .vertices
            .iter()
            .flat_map(|v| {
                let mut b = v.pos[0].to_le_bytes().to_vec();
                b.extend_from_slice(&v.pos[1].to_le_bytes());
                b.extend_from_slice(&v.color);
                b
            })
            .collect();
        assert_eq!(
            rec.buffer(v).expect("vertices"),
            &vertices[..],
            "{what}: vertices"
        );
    } else {
        assert!(c.mesh.indices.is_empty(), "{what}: a mesh without buffers");
    }
}

fn pointer(world: [f64; 2], time: f64) -> Pointer {
    Pointer {
        world,
        screen: [world[0] * VIEW.zoom, world[1] * VIEW.zoom],
        zoom: VIEW.zoom,
        button: Button::Primary,
        kind: PointerKind::Mouse,
        mods: Modifiers::default(),
        time,
    }
}

/// A drag in a few steps, drawing after each move; returns whether a
/// gesture was open during it.
fn drag(s: &mut Session, p: &mut Painter<Recorder>, from: [f64; 2], by: [f64; 2], t: &mut f64) {
    *t += 1000.0;
    s.input(Input::PointerDown(pointer(from, *t)));
    for k in 1..=4 {
        *t += 16.0;
        let f = f64::from(k) / 4.0;
        let at = [from[0] + by[0] * f, from[1] + by[1] * f];
        s.input(Input::PointerMove(pointer(at, *t)));
        p.draw(s, *t);
    }
    *t += 16.0;
    let end = [from[0] + by[0], from[1] + by[1]];
    s.input(Input::PointerUp(pointer(end, *t)));
}

/// 04 §25: after random edits, gestures (moves, rotations, drawing a row
/// block), undo, redo, numbering previews and floor changes, the live scene
/// kept up to date piece by piece equals one built from scratch, and the
/// seat and mesh buffers hold exactly its bytes.
#[test]
fn incremental_equals_from_scratch() {
    let subjects = [
        preset("theatre"),
        cavea_testkit::synthetic::stadium(3_000, 5),
    ];
    for (n, doc) in subjects.into_iter().enumerate() {
        let seed = 7_000 + n as u64;
        let mut r = Rng(seed);
        let mut s = Session::load(doc).expect("load");
        let mut p = painter(VIEW);
        settle(&mut p, &s);
        let mut t = 0.0;
        for step in 0..160 {
            let ids: Vec<u32> = s.document().children(None).to_vec();
            match r.below(12) {
                0..=4 => {
                    let cmd = command(&mut r, s.document());
                    let _ = s.apply(cmd);
                }
                5 => {
                    let _ = s.undo();
                }
                6 => {
                    let _ = s.redo();
                }
                7 | 8 => {
                    // Drag a top-level node by its middle.
                    if let Some(id) = r.pick(&ids)
                        && let Some(b) = s.index().bounds(id)
                    {
                        s.set_tool(ToolId::Select);
                        s.select(Level::Top, [id]);
                        let c = [(b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0];
                        drag(&mut s, &mut p, c, [r.f(400.0), r.f(400.0)], &mut t);
                    }
                }
                9 => {
                    s.set_tool(ToolId::RowBlock);
                    let at = [r.f(3000.0), r.f(3000.0)];
                    drag(&mut s, &mut p, at, [300.0 + r.f(200.0), 200.0], &mut t);
                    s.set_tool(ToolId::Select);
                }
                10 => {
                    s.select_all();
                    let numbering = cavea_edit::Numbering {
                        rows: None,
                        seats: Some(SeatLabels {
                            start: Some(LabelStart::Number(11 + r.below(5) as u32)),
                            ..SeatLabels::default()
                        }),
                        after_z: None,
                        across: None,
                        keep_overrides: false,
                    };
                    let _ = s.preview_numbering(&numbering);
                }
                _ => {
                    let floors: Vec<u32> = s.document().floors().iter().map(|f| f.id).collect();
                    if let Some(f) = r.pick(&floors) {
                        s.set_floor(f);
                    }
                }
            }
            settle(&mut p, &s);
            let what = format!("seed {seed} step {step}");
            let mut fresh = painter(VIEW);
            settle(&mut fresh, &s);
            match (p.engine().scene(), fresh.engine().scene()) {
                (Some(a), Some(b)) => assert_eq!(drawn(a), drawn(b), "{what}"),
                (a, b) => assert_eq!(a.is_some(), b.is_some(), "{what}"),
            }
            if p.engine().scene().is_some() {
                gpu_matches(&p, &what);
            }
        }
        assert!(p.sync().merges > 1);
    }
}

/// 04 §25: while a drag is open nothing is compiled and no seat byte is
/// uploaded; each moved piece is drawn with a transform that puts its
/// seats where compiling it at the new place puts them (within the quarter
/// unit positions are stored in, turned).
#[test]
fn transforms_match_baked_positions() {
    let doc = preset("arena");
    let mut s = Session::load(doc).expect("load");
    let mut p = painter(VIEW);
    settle(&mut p, &s);
    let mut r = Rng(42);
    let mut t = 0.0;
    let mut tested = (0, 0);
    for round in 0..16 {
        let ids: Vec<u32> = s.document().children(None).to_vec();
        let picked: Vec<u32> = (0..1 + r.below(6)).filter_map(|_| r.pick(&ids)).collect();
        s.set_tool(ToolId::Select);
        s.select(Level::Top, picked.iter().copied());
        let overlay = s.overlay();
        // A seat of a node picked whose topmost node there is that node (a
        // section's seat through its child).
        let doc = s.document();
        let tol = 4.0 / VIEW.zoom;
        let on_top = picked.iter().find_map(|&id| {
            let holders = doc.children(Some(id));
            let holders = if holders.is_empty() {
                &[id][..]
            } else {
                holders
            };
            holders.iter().find_map(|&h| {
                let seats = s.index().seats(h).iter().filter(|x| !x.removed);
                seats.take(20).find_map(|x| {
                    let at = s.index().to_world(h, x.pos)?;
                    (s.index().hit(doc, None, at, tol, |_| true) == Some(id)).then_some(at)
                })
            })
        });
        let Some(centre) = on_top else { continue };
        // A move by its body, or a turn by the rotate handle.
        let rotate = round % 2 == 1;
        let from = if rotate {
            let h = (overlay.handles.iter())
                .find(|h| h.kind == cavea_edit::HandleKind::Rotate)
                .expect("rotate handle");
            [
                h.at[0] + h.offset[0] / VIEW.zoom,
                h.at[1] + h.offset[1] / VIEW.zoom,
            ]
        } else {
            centre
        };
        // At least 300 cu (15 CSS px at this zoom): past the drag threshold.
        let by = [300.0 + r.f(250.0).abs(), -(300.0 + r.f(250.0).abs())];
        t += 1000.0;
        s.input(Input::PointerDown(pointer(from, t)));
        t += 16.0;
        s.input(Input::PointerMove(pointer(
            [from[0] + by[0], from[1] + by[1]],
            t,
        )));
        p.draw(&s, t);
        let compiled = p.sync().compiled;
        p.engine_mut().backend_mut().clear();
        t += 16.0;
        let end = [from[0] + by[0] * 1.5, from[1] + by[1] * 1.5];
        s.input(Input::PointerMove(pointer(end, t)));
        assert!(s.editor().gesture_open(), "round {round}: {:?}", s.cursor());
        p.draw(&s, t);
        assert_eq!(
            p.sync().compiled,
            compiled,
            "round {round}: nothing compiled"
        );
        let ([st, dy], _) = p.engine().live_buffers().expect("buffers");
        let seat_writes = (p.engine().backend().writes.iter())
            .filter(|w| w.0 == st.0 || w.0 == dy.0)
            .count();
        assert_eq!(seat_writes, 0, "round {round}: no seat upload");
        // Where the transforms draw each moved piece's seats now.
        let scene = p.engine().scene().expect("scene");
        let pieces = p.engine().pieces();
        let mut moved: Vec<[f64; 2]> = Vec::new();
        for (a, spans) in p.engine().moving() {
            for span in spans {
                for idx in pieces[span].1.seats.clone() {
                    let [x, y] = scene.pos[idx as usize].map(f64::from);
                    moved.push([
                        a[0][0] * x + a[1][0] * y + a[2][0],
                        a[0][1] * x + a[1][1] * y + a[2][1],
                    ]);
                }
            }
        }
        assert!(!moved.is_empty(), "round {round}: something moved");
        t += 16.0;
        s.input(Input::PointerUp(pointer(end, t)));
        settle(&mut p, &s);
        assert!(p.engine().moving().is_empty());
        let baked = p.engine().scene().expect("scene");
        let mut at: Vec<[f64; 2]> = baked.pos.iter().map(|q| q.map(f64::from)).collect();
        // Each transformed seat lies on a baked seat.
        let key = |q: &[f64; 2]| ((q[0] * 4.0).round() as i64, (q[1] * 4.0).round() as i64);
        at.sort_by_key(key);
        for q in &moved {
            let near = at.partition_point(|b| key(b) < (key(q).0 - 2, i64::MIN));
            let hit = at[near..]
                .iter()
                .take_while(|b| key(b).0 <= key(q).0 + 2)
                .any(|b| (b[0] - q[0]).hypot(b[1] - q[1]) <= 0.36);
            assert!(hit, "round {round}: {q:?} has no baked seat within 0.36 cu");
        }
        gpu_matches(&p, &format!("round {round}"));
        if rotate {
            tested.1 += 1;
        } else {
            tested.0 += 1;
        }
    }
    assert!(
        tested.0 >= 4 && tested.1 >= 4,
        "moves and turns tested: {tested:?}"
    );
}

/// 04 §20.2 for the canvas: after a lost context comes back, its first frame
/// makes the scene's resources once from the CPU copies, compiling nothing,
/// and the GPU holds the scene's bytes again.
#[test]
fn context_restores_once() {
    let doc = preset("theatre");
    let s = Session::load(doc).expect("load");
    let mut p = painter(VIEW);
    settle(&mut p, &s);
    let compiled = p.sync().compiled;
    p.handle(Command::ContextLost);
    assert!(p.engine().backend().buffers.is_empty() || p.engine().live_buffers().is_none());
    p.engine_mut().backend_mut().clear();
    // While lost: no frame.
    p.draw(&s, 1.0);
    assert_eq!(p.engine().backend().frames, 0);
    assert!(p.handle(Command::ContextRestored));
    p.draw(&s, 2.0);
    let rec = p.engine().backend();
    assert_eq!(rec.restores, 1);
    assert_eq!(rec.frames, 1);
    // Seats (2), overlay rings (2), mesh (2) once; labels and overlay as
    // drawn.
    let made = rec.created;
    p.engine_mut().backend_mut().clear();
    p.draw(&s, 3.0);
    assert_eq!(
        p.engine().backend().created,
        0,
        "made {made} once, then none"
    );
    assert_eq!(p.sync().compiled, compiled, "nothing compiled");
    gpu_matches(&p, "restored");
}

/// 04 §18: a frame that changes nothing draws nothing and uploads nothing.
#[test]
fn idle_frames_cost_nothing() {
    let s = Session::load(preset("theatre")).expect("load");
    let mut p = painter(VIEW);
    settle(&mut p, &s);
    p.engine_mut().backend_mut().clear();
    for t in 0..10 {
        assert!(!p.draw(&s, 1e4 + f64::from(t)));
    }
    let rec = p.engine().backend();
    assert_eq!((rec.frames, rec.uploaded), (0, 0));
}

/// Designer targets (01 §5), measured natively with the recording backend:
/// `cargo test --release -p cavea-designer measure_canvas -- --ignored
/// --nocapture`. The engine's CPU time and what it uploads and draws; no GPU.
#[test]
#[ignore]
fn measure_canvas() {
    use std::time::Instant;
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    let doc = preset("stadium-100k");
    let mut s = Session::load(doc).expect("load");
    let size = s.document().settings().seat_size;
    let fit = s.fit().expect("fit");
    let centre = [
        (fit.min[0] + fit.max[0]) / 2.0,
        (fit.min[1] + fit.max[1]) / 2.0,
    ];
    let base = View {
        centre,
        zoom: (1920.0 - 48.0) / (fit.max[0] - fit.min[0]),
        width: 1920.0,
        height: 1080.0,
        dpr: 1.0,
    };
    let mut p = painter(base);
    let t0 = Instant::now();
    p.draw(&s, 0.0);
    println!(
        "first draw of stadium-100k from the opened draft: {:.1} ms ({} pieces, {} seats compiled, {} KB uploaded)",
        ms(t0),
        p.sync().compiled,
        p.sync().compiled_seats,
        p.engine().backend().uploaded / 1024
    );
    settle(&mut p, &s);

    // Draw calls at overview, seats and detail, without and with a selection.
    let sec = s.document().children(None)[0];
    let b = s.index().bounds(sec).expect("bounds");
    let at = [(b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0];
    for (name, px) in [("overview", 0.0), ("seats", 8.0), ("detail", 24.0)] {
        let view = if px == 0.0 {
            base
        } else {
            View {
                centre: at,
                zoom: px / size,
                ..base
            }
        };
        let mut counts = Vec::new();
        for selected in [false, true] {
            if selected {
                s.select_all();
            } else {
                s.select(Level::Top, []);
            }
            p.set_view(view);
            settle(&mut p, &s);
            p.engine_mut().backend_mut().clear();
            p.set_view(View {
                centre: [view.centre[0] + 1.0, view.centre[1]],
                ..view
            });
            p.draw(&s, 1.0);
            counts.push(p.engine().backend().draw_calls());
        }
        println!(
            "draw calls at {name}: {} without a selection, {} with everything selected",
            counts[0], counts[1]
        );
    }
    s.select(Level::Top, []);
    p.set_view(base);
    settle(&mut p, &s);

    // A frame that changes nothing.
    p.engine_mut().backend_mut().clear();
    let t0 = Instant::now();
    let again = p.draw(&s, 2.0);
    println!(
        "a frame that changes nothing: {:.4} ms, {} draws, {} bytes uploaded (again: {again})",
        ms(t0),
        p.engine().backend().draws.len(),
        p.engine().backend().uploaded
    );

    // A 5,000-seat block: its seat pitch changed; dragged.
    let draft = cavea_edit::Draft::RowBlock {
        origin: [fit.max[0] + 2000.0, fit.min[1]],
        rows: 50,
        seats: 100,
        seat_pitch: 50.0,
        row_pitch: 90.0,
    };
    let block = s.insert(None, &draft).expect("block");
    settle(&mut p, &s);
    s.select(Level::Top, [block]);
    let c0 = p.sync().compiled_seats;
    p.engine_mut().backend_mut().clear();
    let t0 = Instant::now();
    s.set_field(&cavea_edit::Field::SeatPitch(52.0))
        .expect("pitch");
    let edit = ms(t0);
    let t0 = Instant::now();
    p.draw(&s, 3.0);
    let rec = p.engine().backend();
    println!(
        "seat pitch of a 5,000-seat block: edit {edit:.2} ms, then draw {:.2} ms: {} seats compiled, {} KB uploaded in {} writes",
        ms(t0),
        p.sync().compiled_seats - c0,
        rec.uploaded / 1024,
        rec.writes.len()
    );
    for (what, ids) in [
        ("one 5,000-seat block", vec![block]),
        (
            "everything (191 top-level nodes)",
            s.document().children(None).to_vec(),
        ),
    ] {
        settle(&mut p, &s);
        s.select(Level::Top, ids.iter().copied());
        // Press on a seat of the first node and drag.
        let first = ids[0];
        let holder = s
            .document()
            .children(Some(first))
            .first()
            .copied()
            .unwrap_or(first);
        let seat = s
            .index()
            .seats(holder)
            .iter()
            .find(|x| !x.removed)
            .expect("seat")
            .pos;
        let from = s.index().to_world(holder, seat).expect("world");
        let zoom = base.zoom;
        let ptr = |w: [f64; 2], t: f64| Pointer {
            world: w,
            screen: [w[0] * zoom, w[1] * zoom],
            zoom,
            button: Button::Primary,
            kind: PointerKind::Mouse,
            mods: Modifiers::default(),
            time: t,
        };
        s.input(Input::PointerDown(ptr(from, 1e6)));
        s.input(Input::PointerMove(ptr(
            [from[0] + 400.0, from[1]],
            1e6 + 16.0,
        )));
        p.draw(&s, 4.0);
        let compiled = p.sync().compiled;
        p.engine_mut().backend_mut().clear();
        let steps = 20;
        let (mut input, mut draw) = (0.0, 0.0);
        for k in 1..=steps {
            let w = [
                from[0] + 400.0 + f64::from(k) * 37.0,
                from[1] + f64::from(k) * 11.0,
            ];
            let t0 = Instant::now();
            s.input(Input::PointerMove(ptr(w, 1e6 + 16.0 * f64::from(k + 1))));
            input += ms(t0);
            let t0 = Instant::now();
            p.draw(&s, 5.0 + f64::from(k));
            draw += ms(t0);
        }
        let rec = p.engine().backend();
        println!(
            "drag step of {what}: input {:.3} ms, draw {:.3} ms (engine CPU), {} compiled, {} B uploaded over {steps} steps, {} draws a frame",
            input / f64::from(steps),
            draw / f64::from(steps),
            p.sync().compiled - compiled,
            rec.uploaded,
            rec.draw_calls() / steps as usize
        );
        let end = [from[0] + 400.0 + 20.0 * 37.0, from[1] + 220.0];
        s.input(Input::PointerUp(ptr(end, 2e6)));
        let t0 = Instant::now();
        let mut frames = 0;
        let mut worst = 0.0f64;
        let mut each = Vec::new();
        loop {
            let t1 = Instant::now();
            let up = p.engine().backend().uploaded;
            let made = p.engine().backend().created;
            p.draw(&s, 100.0 + f64::from(frames));
            worst = worst.max(ms(t1));
            let kb = (p.engine().backend().uploaded - up) / 1024;
            let made = p.engine().backend().created - made;
            // Each buffer made ends with `getError`, which waits for the GPU.
            each.push(format!("{:.1} ms {kb} KB {made} buffers", ms(t1)));
            frames += 1;
            if !p.sync().pending() || frames > 100 {
                break;
            }
        }
        println!(
            "  baked on release over {frames} frames, {:.1} ms in all, the longest {worst:.1} ms ({})",
            ms(t0),
            each.join(", ")
        );
    }

    // The overlay batcher's build with the worst overlay: everything selected
    // (191 outlines, 180 dashed section outlines, handles) with a user guide
    // per 100 cu, at overview, rebuilt for a new view.
    s.select_all();
    for i in 0..60 {
        let _ = s.add_guide(cavea_edit::UserGuide {
            axis: if i % 2 == 0 {
                cavea_edit::GuideAxis::Vertical
            } else {
                cavea_edit::GuideAxis::Horizontal
            },
            at: f64::from(i) * 100.0,
        });
    }
    settle(&mut p, &s);
    let overlay = s.overlay();
    let shapes = crate::paint::shapes(&overlay, &crate::paint::LIGHT);
    let t0 = Instant::now();
    let mut batch = cavea_render::overlay::Batch::default();
    batch.set(shapes.clone());
    let mut rec = Recorder::default();
    let mut atlas = cavea_render::text::atlas::TextAtlas::default();
    let mut events = Vec::new();
    let camera = cavea_render::camera::Camera {
        centre,
        zoom: base.zoom,
        rotation: [1.0, 0.0],
    };
    let size = cavea_render::camera::Size {
        width: 1920.0,
        height: 1080.0,
    };
    batch
        .prepare(&mut rec, &mut atlas, "sans", camera, size, 1.0, &mut events)
        .expect("batch");
    let st = batch.stats();
    println!(
        "overlay batch of everything selected ({} shapes): {:.2} ms, {} triangles, {} vertices",
        shapes.len(),
        ms(t0),
        st.indices / 3,
        st.vertices
    );
    use cavea_render::overlay::Shape as S;
    type Pick = fn(&S) -> bool;
    let kinds: [(&str, Pick); 4] = [
        ("lines", |s| matches!(s, S::Line { dash: None, .. })),
        ("dashed", |s| matches!(s, S::Line { dash: Some(_), .. })),
        ("handles", |s| matches!(s, S::Handle { .. })),
        ("guides", |s| matches!(s, S::Guide { .. })),
    ];
    for (kind, pick) in kinds {
        let part: Vec<S> = shapes.iter().filter(|s| pick(s)).cloned().collect();
        let mut b = cavea_render::overlay::Batch::default();
        let n = part.len();
        b.set(part);
        b.prepare(&mut rec, &mut atlas, "sans", camera, size, 1.0, &mut events)
            .expect("batch");
        println!("  {kind}: {n} shapes, {} triangles", b.stats().indices / 3);
    }
    let t0 = Instant::now();
    let _ = s.overlay();
    println!("overlay() with everything selected: {:.3} ms", ms(t0));
}

/// 04 §11 with transforms: while every other section is dragged, with more
/// ranges that do not move than draw calls allowed, joining them never
/// takes in moved seats, so no seat is drawn twice in a frame (once moved,
/// once where it was).
#[test]
fn moved_seats_are_drawn_once() {
    let doc = preset("stadium-100k");
    let mut s = Session::load(doc).expect("load");
    let fit = s.fit().expect("fit");
    let size = s.document().settings().seat_size;
    // The whole chart in view at the seats level: a very large canvas.
    let zoom = 6.0 / size;
    let view = View {
        centre: [
            (fit.min[0] + fit.max[0]) / 2.0,
            (fit.min[1] + fit.max[1]) / 2.0,
        ],
        zoom,
        width: (fit.max[0] - fit.min[0]) * zoom + 100.0,
        height: (fit.max[1] - fit.min[1]) * zoom + 100.0,
        dpr: 1.0,
    };
    let mut p = painter(view);
    settle(&mut p, &s);
    let roots: Vec<u32> = s.document().children(None).to_vec();
    let every_other: Vec<u32> = roots.iter().copied().step_by(2).collect();
    s.set_tool(ToolId::Select);
    s.select(Level::Top, every_other.iter().copied());
    let first = every_other[0];
    let holder = s
        .document()
        .children(Some(first))
        .first()
        .copied()
        .unwrap_or(first);
    let seat = s
        .index()
        .seats(holder)
        .iter()
        .find(|x| !x.removed)
        .expect("seat")
        .pos;
    let from = s.index().to_world(holder, seat).expect("world");
    let ptr = |w: [f64; 2], t: f64| Pointer {
        world: w,
        screen: [w[0] * zoom, w[1] * zoom],
        zoom,
        button: Button::Primary,
        kind: PointerKind::Mouse,
        mods: Modifiers::default(),
        time: t,
    };
    s.input(Input::PointerDown(ptr(from, 1e6)));
    s.input(Input::PointerMove(ptr(
        [from[0] + 300.0, from[1] + 200.0],
        1e6 + 16.0,
    )));
    p.engine_mut().backend_mut().clear();
    p.draw(&s, 1.0);
    assert!(s.editor().gesture_open());
    assert!(!p.engine().moving().is_empty(), "pieces are moved");
    let ([st, dy], _) = p.engine().live_buffers().expect("buffers");
    let seat_draws: Vec<_> = (p.engine().backend().draws.iter())
        .filter(|d| d.buffers == [Some(st), Some(dy)])
        .collect();
    let mut drawn: Vec<std::ops::Range<u32>> =
        seat_draws.iter().flat_map(|d| d.ranges.clone()).collect();
    drawn.sort_by_key(|r| r.start);
    for w in drawn.windows(2) {
        assert!(
            w[0].end <= w[1].start,
            "seats drawn twice: {:?} and {:?}",
            w[0],
            w[1]
        );
    }
    // 90 sections stay, each its own range between moved ones: none can be
    // joined without taking in moved seats, so the 64-call target gives way
    // for the drag's duration.
    let stationary = seat_draws.first().map_or(0, |d| d.ranges.len());
    assert_eq!(stationary, 90);
}

/// 04 §25: seat labels the baked atlas cannot draw (here "Sitz 1", five
/// glyphs and lower case) come from the dynamic atlas as text instances;
/// while their block is dragged they are built where its seats are drawn,
/// not where it was compiled.
#[test]
fn dynamic_seat_labels_move_with_their_piece() {
    use cavea_render::protocol::Event;
    let mut s = Session::load(preset("theatre")).expect("load");
    let block = s
        .insert(
            None,
            &cavea_edit::Draft::RowBlock {
                origin: [20_000.0, 0.0],
                rows: 2,
                seats: 4,
                seat_pitch: 60.0,
                row_pitch: 100.0,
            },
        )
        .expect("block");
    s.select(Level::Top, [block]);
    let numbering = cavea_edit::Numbering {
        rows: None,
        seats: Some(SeatLabels {
            prefix: "Sitz ".into(),
            ..SeatLabels::default()
        }),
        after_z: None,
        across: None,
        keep_overrides: false,
    };
    s.number(&numbering).expect("number");
    let view = View {
        centre: [20_000.0, 50.0],
        zoom: 0.5,
        width: 2400.0,
        height: 800.0,
        dpr: 1.0,
    };
    let mut p = painter(view);
    let [w, h] = cavea_render::text::ATLAS_SIZE;
    p.handle(Command::SetGlyphAtlas {
        rgba: vec![0; (w * h * 4) as usize],
    });
    // Draws, answering every text request with a small raster.
    let frames = |p: &mut Painter<Recorder>, s: &Session, t: f64| {
        for k in 0..40 {
            p.draw(s, t + f64::from(k));
            for e in p.take_events() {
                if let Event::TextRequest { id, .. } = e {
                    p.handle(Command::SetText {
                        id,
                        width: 24,
                        height: 12,
                        px: 11,
                        pixels: vec![255; 24 * 12],
                    });
                }
            }
        }
    };
    frames(&mut p, &s, 0.0);
    // The text instances' anchors in the last frame drawn.
    let anchors = |p: &mut Painter<Recorder>, s: &Session, t: f64| {
        p.engine_mut().backend_mut().clear();
        // A frame is drawn only for a change: move the view a little.
        p.set_view(View {
            centre: [view.centre[0] + t * 1e-4, view.centre[1]],
            ..view
        });
        frames(p, s, t);
        let rec = p.engine().backend();
        let mut out = Vec::new();
        let last = rec
            .draws
            .iter()
            .rposition(|d| d.pipeline == cavea_render::backend::Pipeline::Mesh);
        let from = last.map_or(0, |i| i.saturating_sub(12));
        for d in rec.draws.iter().skip(from) {
            if d.pipeline != cavea_render::backend::Pipeline::Text {
                continue;
            }
            let Some(bytes) = d.buffers[0].and_then(|b| rec.buffer(b)) else {
                continue;
            };
            for r in &d.ranges {
                for i in r.clone() {
                    let at = i as usize * 36;
                    let f = |o: usize| {
                        f32::from_le_bytes(bytes[at + o..at + o + 4].try_into().expect("4"))
                    };
                    out.push([f(0), f(4)]);
                }
            }
        }
        out
    };
    let before = anchors(&mut p, &s, 100.0);
    let near = |a: &[[f32; 2]], x: f32| a.iter().filter(|p| (p[0] - x).abs() < 200.0).count();
    // Eight seat labels and the rows' four end labels.
    assert_eq!(near(&before, 20_000.0), 12, "{before:?}");
    // Drag the block 600 cu right.
    let seat = s.index().seats(block)[0].pos;
    let from = s.index().to_world(block, seat).expect("world");
    let ptr = |w: [f64; 2], t: f64| Pointer {
        world: w,
        screen: [w[0] * 0.5, w[1] * 0.5],
        zoom: 0.5,
        button: Button::Primary,
        kind: PointerKind::Mouse,
        mods: Modifiers::default(),
        time: t,
    };
    s.input(Input::PointerDown(ptr(from, 1e6)));
    s.input(Input::PointerMove(ptr(
        [from[0] + 600.0, from[1]],
        1e6 + 16.0,
    )));
    assert!(s.editor().gesture_open());
    let during = anchors(&mut p, &s, 200.0);
    assert!(!p.engine().moving().is_empty());
    assert_eq!(near(&during, 20_000.0), 0, "none left behind: {during:?}");
    assert_eq!(near(&during, 20_600.0), 12, "all moved: {during:?}");

    // Released, the block is baked and its seats' `idx` shift: the seat
    // labels' rasters, keyed by `idx`, are asked for again, and nothing
    // else is (the row labels stay drawn rather than blinking out).
    s.input(Input::PointerUp(ptr(
        [from[0] + 600.0, from[1]],
        1e6 + 32.0,
    )));
    assert!(!s.editor().gesture_open());
    let mut asked = Vec::new();
    for k in 0..40 {
        p.draw(&s, 300.0 + f64::from(k));
        for e in p.take_events() {
            if let Event::TextRequest { id, text, .. } = e {
                asked.push(text);
                p.handle(Command::SetText {
                    id,
                    width: 24,
                    height: 12,
                    px: 11,
                    pixels: vec![255; 24 * 12],
                });
            }
        }
    }
    assert!(p.engine().moving().is_empty(), "baked");
    assert!(asked.iter().any(|t| t.starts_with("Sitz ")), "{asked:?}");
    assert!(asked.iter().all(|t| t.starts_with("Sitz ")), "{asked:?}");
}

/// 03 §2.7, 07 §4: a reference image is the designer's only. The canvas
/// draws it (the live scene's `IMGS`, behind everything) and asks the host
/// for its picture, and a drag moves it with the pointer, not only once
/// baked; the chart the preview shows and the server would compile has no
/// image at all. Made a background image, the compiled chart has it.
#[test]
fn reference_images_are_drawn_but_never_compiled() {
    use cavea_compiler::document::ImageRole;
    use cavea_edit::{Draft, Field};
    use cavea_format::Artefact;
    use cavea_render::protocol::Event;

    let hash = "00112233445566778899aabbccddeeff";
    let mut s = Session::load(preset("theatre")).expect("load");
    s.set_snap(cavea_edit::SnapSettings {
        grid: false,
        step: 5.0,
        objects: false,
        spacing: false,
        rows: false,
        angles: false,
        guides: false,
    });
    let draft = Draft::Image {
        center: [0.0, 0.0],
        size: [4000.0, 3000.0],
        asset: hash.into(),
        role: ImageRole::Reference,
    };
    s.insert(None, &draft).expect("insert");
    let mut p = painter(VIEW);
    settle(&mut p, &s);
    let shown = |p: &Painter<Recorder>| -> Vec<(String, [i32; 4])> {
        let scene = p.engine().scene().expect("scene");
        let c = &scene.chart;
        (c.images.iter())
            .map(|i| {
                let asset = c.strings.get(i.asset).unwrap_or_default().to_owned();
                (asset, i.bounds)
            })
            .collect()
    };
    // Bounds in quarter units, centred on the image's position.
    assert_eq!(
        shown(&p),
        vec![(hash.to_owned(), [-8000, -6000, 8000, 6000])]
    );
    let request = |p: &mut Painter<Recorder>| {
        p.take_events().into_iter().find_map(|e| match e {
            Event::ImageRequest { id, asset, .. } if asset == hash => Some(id),
            _ => None,
        })
    };
    let id = request(&mut p).expect("the picture is asked for");
    // Not loaded (the bytes are not to be had): not asked again by the
    // live scene, until the host has the bytes and says so.
    p.set_image(id, None);
    settle(&mut p, &s);
    assert_eq!(request(&mut p), None);
    assert!(p.retry_images());
    settle(&mut p, &s);
    assert!(request(&mut p).is_some(), "asked again once retried");

    let compiled = |s: &Session| {
        let bytes = crate::preview::chart(s).expect("compiles");
        cavea_format::decode(&bytes, Artefact::Public)
            .expect("decode")
            .images
            .len()
    };
    assert_eq!(compiled(&s), 0, "a reference image is never compiled");

    // A drag moves it at every step: the picture follows the pointer.
    let mut t = 0.0;
    t += 1000.0;
    s.input(Input::PointerDown(pointer([1500.0, 1000.0], t)));
    for k in 1..=3 {
        t += 16.0;
        let at = [1500.0 + 400.0 * f64::from(k), 1000.0];
        s.input(Input::PointerMove(pointer(at, t)));
        p.draw(&s, t);
        let dx = 1600 * k;
        assert_eq!(shown(&p)[0].1, [-8000 + dx, -6000, 8000 + dx, 6000]);
    }
    t += 16.0;
    s.input(Input::PointerUp(pointer([2700.0, 1000.0], t)));
    settle(&mut p, &s);

    s.set_field(&Field::ImageRole(ImageRole::Background))
        .expect("role");
    assert_eq!(compiled(&s), 1);
    settle(&mut p, &s);
    assert_eq!(shown(&p).len(), 1);
}
