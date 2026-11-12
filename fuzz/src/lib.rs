//! Shared parts of the fuzz targets (`docs/13-testing-benchmarks.md` §5): the
//! fixed chart and the input format of `snapshot_apply`, and a GPU backend that
//! asserts every write and draw stays inside what it created.
//!
//! Not a member of the main workspace: `cargo fuzz` builds it on the nightly
//! toolchain of `nix develop .#fuzz`. Assertions are how the targets report.

pub mod http;

use std::collections::BTreeMap;
use std::sync::OnceLock;

use cavea_render::Engine;
use cavea_render::backend::{
    BackendError, BufferDesc, BufferId, Caps, DrawCall, GpuBackend, Pipeline, Rect, ResourceId,
    Sampling, Target, TextureDesc, TextureFormat, TextureId,
};
use cavea_render::protocol::{
    Command, ErrorCode, Event, HoldOutcome, ObjectFailure, ObjectRef, SessionMode,
};
use cavea_render::seat::{DYNAMIC_STRIDE, STATIC_STRIDE};

/// Two floors: two sections of three rows on the first, a row outside any
/// section on the second (90 + 12 seats).
pub const CHART: &str = r##"{"schema":1,"name":"fuzz","nextOid":1000,
  "categories":[{"key":"a","label":"A","color":"#2563EB"},{"key":"b","label":"B","color":"#DC2626","accessible":true}],
  "floors":[{"id":1,"name":"Ground","focalPoint":[0,-500]},{"id":2,"name":"Upper","level":1}],
  "nodes":[
    {"id":10,"type":"section","floor":1,"label":"A","category":"a","children":[{"id":11,"type":"rowBlock",
      "rows":[{"rid":12,"seats":15,"oids":[[100,15]]},{"rid":13,"seats":15,"oids":[[200,15]],"removed":[7]},{"rid":14,"seats":16,"oids":[[300,16]]}]}]},
    {"id":20,"type":"section","floor":1,"label":"B","category":"b","origin":[1200,0],"rotation":20,"children":[{"id":21,"type":"rowBlock",
      "curve":{"kind":"arc","radius":900},"rows":[{"rid":22,"seats":15,"oids":[[400,15]]},{"rid":23,"seats":15,"oids":[[500,15]]},{"rid":24,"seats":15,"oids":[[600,15]]}]}]},
    {"id":30,"type":"row","floor":2,"label":"Z","category":"a","path":{"kind":"polyline","points":[[0,0],[600,0]]},"seats":12,"oids":[[700,12]]}]}"##;

/// The public artefact of the fixed chart, compiled once per process.
pub fn chart() -> &'static [u8] {
    static CCF: OnceLock<Vec<u8>> = OnceLock::new();
    CCF.get_or_init(|| {
        let doc = cavea_compiler::ChartDocument::from_json(CHART.as_bytes())
            .expect("the fixed chart parses");
        cavea_compiler::compile(&doc)
            .expect("the fixed chart compiles")
            .public
    })
}

/// One step of a `snapshot_apply` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// `LoadChart` with [`chart`].
    Load,
    /// Server frame bytes through `Command::from_frame`.
    Frame(Vec<u8>),
    Select(u16),
    Deselect(u16),
    Clear,
    /// The result of the last `HoldRequest` (`request: None`) or of a made-up
    /// one; a failure names `object` with `code`.
    HoldResult {
        request: Option<u8>,
        failure: Option<(u16, &'static str)>,
    },
    Tick,
    /// Manual (true) or hold (false) session.
    Manual(bool),
}

const CODES: [&str; 3] = ["not_free", "not_found", "rate_limited"];

/// Splits fuzzer bytes into steps. Every byte string is a valid input; a
/// step cut short by the end reads zeros.
pub fn ops(mut data: &[u8]) -> Vec<Op> {
    let mut take = |n: usize| {
        let (head, rest) = data.split_at(n.min(data.len()));
        data = rest;
        let mut out = [0u8; 2];
        out[..head.len().min(2)].copy_from_slice(&head[..head.len().min(2)]);
        (head.to_vec(), out)
    };
    let mut out = Vec::new();
    loop {
        let (tag, _) = take(1);
        let Some(&tag) = tag.first() else {
            return out;
        };
        out.push(match tag % 8 {
            0 => Op::Load,
            1 => {
                let (_, len) = take(2);
                Op::Frame(take(usize::from(u16::from_le_bytes(len))).0)
            }
            2 => Op::Select(u16::from_le_bytes(take(2).1)),
            3 => Op::Deselect(u16::from_le_bytes(take(2).1)),
            4 => Op::Clear,
            5 => {
                let [request, kind] = take(2).1;
                let object = u16::from_le_bytes(take(2).1);
                Op::HoldResult {
                    request: request.checked_sub(1),
                    failure: match kind % 4 {
                        0 => None,
                        k => Some((object, CODES[usize::from(k - 1)])),
                    },
                }
            }
            6 => Op::Tick,
            _ => Op::Manual(take(1).1[0] & 1 != 0),
        });
    }
}

/// The inverse of [`ops`], for writing seeds.
pub fn encode(ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::new();
    for op in ops {
        match op {
            Op::Load => out.push(0),
            Op::Frame(frame) => {
                out.push(1);
                let len = u16::try_from(frame.len()).expect("frame under 64 KiB");
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(frame);
            }
            Op::Select(i) | Op::Deselect(i) => {
                out.push(if matches!(op, Op::Select(_)) { 2 } else { 3 });
                out.extend_from_slice(&i.to_le_bytes());
            }
            Op::Clear => out.push(4),
            Op::HoldResult { request, failure } => {
                let kind = failure.map_or(0, |(_, c)| {
                    1 + CODES.iter().position(|&x| x == c).expect("known code") as u8
                });
                let request = request.map_or(0, |r| r.checked_add(1).expect("request below 255"));
                out.extend_from_slice(&[5, request, kind]);
                out.extend_from_slice(&failure.map_or(0, |(o, _)| o).to_le_bytes());
            }
            Op::Tick => out.push(6),
            Op::Manual(on) => out.extend_from_slice(&[7, u8::from(*on)]),
        }
    }
    out
}

/// Runs `ops` through a fresh engine on a [`Checked`] backend. Panics if the
/// engine writes or draws outside a resource, or reports a GPU error.
pub fn run(ops: &[Op]) {
    let mut e = Engine::new(Checked::default());
    e.handle(Command::Resize {
        width: 800.0,
        height: 600.0,
        dpr: 1.0,
        max_dpr: 2.0,
    });
    let mut now = 0.0;
    let mut last_request = None;
    for op in ops.iter().chain([&Op::Tick]) {
        let objects = e.objects().and_then(|n| u32::try_from(n).ok());
        let pick = |i: u16| ObjectRef::Idx(u32::from(i) % objects.map_or(1, |n| n + 2));
        let command = match op {
            Op::Load => Command::LoadChart {
                ccf: chart().to_vec(),
            },
            Op::Frame(frame) => match Command::from_frame(frame, objects) {
                Ok(Some(command)) => {
                    assert!(
                        matches!(
                            command,
                            Command::SetSnapshot { .. }
                                | Command::ApplyDelta { .. }
                                | Command::SetPrivateHolds { .. }
                        ),
                        "a frame gives a state command: {command:?}"
                    );
                    command
                }
                Ok(None) | Err(_) => continue,
            },
            Op::Select(i) => Command::Select {
                objects: vec![pick(*i)],
            },
            Op::Deselect(i) => Command::Deselect {
                objects: vec![pick(*i)],
            },
            Op::Clear => Command::ClearSelection,
            Op::HoldResult { request, failure } => Command::HoldResult {
                request: request.map_or(last_request.unwrap_or(0), u32::from),
                outcome: match failure {
                    None => HoldOutcome::Ok,
                    Some((i, code)) => HoldOutcome::Failed(vec![ObjectFailure {
                        object: pick(*i),
                        code: (*code).to_owned(),
                    }]),
                },
            },
            Op::Tick => {
                now += 16.0;
                Command::Tick { time_ms: now }
            }
            Op::Manual(on) => Command::SetSessionMode(if *on {
                SessionMode::Manual
            } else {
                SessionMode::Hold
            }),
        };
        e.handle(command);
        for event in e.drain_events() {
            match event {
                Event::Error {
                    code: ErrorCode::Gpu,
                    message,
                } => panic!("GPU error: {message}"),
                Event::HoldRequest { request, .. } => last_request = Some(request),
                _ => {}
            }
        }
    }
}

/// A backend that keeps each resource's size and asserts that every update
/// and every drawn instance range lies inside it.
#[derive(Default)]
pub struct Checked {
    next: u32,
    buffers: BTreeMap<u32, usize>,
    textures: BTreeMap<u32, TextureDesc>,
}

impl GpuBackend for Checked {
    /// An image is its size, as in the renderer's recording backend.
    type Image = [u32; 2];

    fn image_size(image: &[u32; 2]) -> [u32; 2] {
        *image
    }

    fn create_image_texture(
        &mut self,
        &[width, height]: &[u32; 2],
    ) -> Result<TextureId, BackendError> {
        let desc = TextureDesc {
            width,
            height,
            format: TextureFormat::Rgba8,
            sampling: Sampling::Linear,
        };
        self.create_texture(desc, None)
    }

    fn caps(&self) -> Caps {
        Caps {
            max_texture_size: 4096,
        }
    }

    fn create_buffer(&mut self, _: BufferDesc, data: &[u8]) -> Result<BufferId, BackendError> {
        self.next += 1;
        self.buffers.insert(self.next, data.len());
        Ok(BufferId(self.next))
    }

    fn update_buffer(
        &mut self,
        id: BufferId,
        offset: u32,
        data: &[u8],
    ) -> Result<(), BackendError> {
        let len = self.buffers.get(&id.0).expect("update of a live buffer");
        let end = offset as usize + data.len();
        assert!(end <= *len, "buffer write {offset}..{end} past {len} B");
        Ok(())
    }

    fn create_texture(
        &mut self,
        desc: TextureDesc,
        data: Option<&[u8]>,
    ) -> Result<TextureId, BackendError> {
        let size =
            desc.width as usize * desc.height as usize * desc.format.bytes_per_pixel() as usize;
        assert!(
            data.is_none_or(|d| d.len() == size),
            "texture data of the wrong size"
        );
        self.next += 1;
        self.textures.insert(self.next, desc);
        Ok(TextureId(self.next))
    }

    fn update_texture(&mut self, id: TextureId, r: Rect, data: &[u8]) -> Result<(), BackendError> {
        let d = self.textures.get(&id.0).expect("update of a live texture");
        assert!(
            r.x + r.width <= d.width && r.y + r.height <= d.height,
            "texture write outside"
        );
        let size = r.width as usize * r.height as usize * d.format.bytes_per_pixel() as usize;
        assert_eq!(data.len(), size, "texture write of the wrong size");
        Ok(())
    }

    fn begin_frame(&mut self, _: Target, _: [f32; 4]) {}

    fn draw(&mut self, call: &DrawCall<'_>) -> Result<(), BackendError> {
        for id in call.buffers.iter().flatten() {
            assert!(self.buffers.contains_key(&id.0), "draw with a dead buffer");
        }
        for id in call.textures.iter().flatten() {
            assert!(
                self.textures.contains_key(&id.0),
                "draw with a dead texture"
            );
        }
        // The shape pipeline's instance ranges against its two buffers; the
        // mesh, glyph and text pipelines are checked for live resources only.
        if call.pipeline == Pipeline::Shape {
            let instances = |slot: Option<BufferId>, stride: usize| {
                let id = slot.expect("shape draws bind both instance buffers");
                self.buffers.get(&id.0).expect("draw with a live buffer") / stride
            };
            let n = instances(call.buffers[0], STATIC_STRIDE)
                .min(instances(call.buffers[1], DYNAMIC_STRIDE));
            for r in call.ranges {
                assert!(
                    r.start <= r.end && r.end as usize <= n,
                    "draw {r:?} past {n} instances"
                );
            }
        }
        Ok(())
    }

    fn end_frame(&mut self) {}

    fn restore(&mut self) -> Result<(), BackendError> {
        self.buffers.clear();
        self.textures.clear();
        Ok(())
    }

    fn destroy(&mut self, id: ResourceId) {
        let live = match id {
            ResourceId::Buffer(b) => self.buffers.remove(&b.0).is_some(),
            ResourceId::Texture(t) => self.textures.remove(&t.0).is_some(),
        };
        assert!(live, "destroy of a dead resource");
    }
}
