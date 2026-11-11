//! The engine protocol (`docs/04-renderer.md` §3, `docs/DECISIONS.md` D-21).
//! The engine is driven only by [`Command`]s and emits only [`Event`]s. Both are
//! plain owned data with no handles, references or closures, so the engine can
//! later run in a worker or behind FFI unchanged. A wire encoding is added when
//! the first such boundary exists.

use cavea_format::wire::{ServerFrame, View, WireError, decode_server};

/// A rectangle in chart units (04 §13 targets, bounds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// An object named by its label or by its index in the compiled chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectRef {
    Label(String),
    Idx(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusChange {
    pub idx: u32,
    pub status: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerType {
    Mouse,
    Touch,
    Pen,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Which gestures move the camera (04 §14, `gestureHandling` in 05 §3.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GestureHandling {
    /// The map takes every gesture.
    #[default]
    Greedy,
    /// One-finger touch drags and plain wheel go to the page; two fingers,
    /// ctrl+wheel and mouse drags move the map.
    Cooperative,
    /// No gesture moves the camera. Taps, long presses and hover still work.
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CameraTarget {
    Bounds(Bounds),
    Object(ObjectRef),
    Section(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectFailure {
    pub object: ObjectRef,
    /// Machine code, as returned by the server.
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoldOutcome {
    Ok,
    Failed(Vec<ObjectFailure>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryPrice {
    pub category: String,
    /// Already formatted for display.
    pub price: String,
}

/// A price range in minor currency units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceRange {
    pub min: u64,
    pub max: u64,
}

/// Whether a selection asks the host to hold (04 §16, `session` in 05 §4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionMode {
    /// A tap selects at once and emits `HoldRequest`; deselecting emits
    /// `ReleaseRequest`.
    #[default]
    Hold,
    /// No hold or release requests: the host holds at checkout.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Picker,
    Static,
    Spotlight,
    Manager,
}

/// The built-in themes (04 §19).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BuiltinTheme {
    #[default]
    Light,
    Dark,
    HighContrast,
}

/// One theme token: its 05 §8 name, with or without the `--cavea-` prefix,
/// and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeToken {
    pub name: String,
    pub value: String,
}

/// Input to the engine (04 §3).
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// A public (or full) CCF artefact. Replaces any chart already loaded;
    /// answered with `Ready`, or an `Error` with `BadChart` that leaves the
    /// engine as it was.
    LoadChart {
        ccf: Vec<u8>,
    },
    /// The public state of every object (04 §17): one code per object, in
    /// `idx` order, 0 available or 1 unavailable. Kept until a chart is
    /// loaded if none is.
    SetSnapshot {
        seq: u64,
        state: Vec<u8>,
    },
    /// Changes in strictly increasing `idx`, applied only at `from_seq`.
    ApplyDelta {
        from_seq: u64,
        to_seq: u64,
        changes: Vec<StatusChange>,
    },
    /// Every object the buyer's hold token holds, and the token's expiry
    /// (Unix ms); replaces the previous set.
    SetPrivateHolds {
        objects: Vec<u32>,
        expires_at_ms: u64,
    },
    /// Canvas size in CSS pixels, the device pixel ratio, and the highest
    /// ratio to render at (`maxDpr`, 04 §21). Applied at the next `Tick`.
    Resize {
        width: f32,
        height: f32,
        dpr: f32,
        max_dpr: f32,
    },
    /// Position in CSS pixels relative to the canvas; time on the same clock as
    /// `Tick`. Queued and applied at the next `Tick` (04 §18).
    Pointer {
        kind: PointerKind,
        id: u32,
        pointer_type: PointerType,
        x: f32,
        y: f32,
        time_ms: f64,
    },
    /// Delta in CSS pixels (the host converts lines and pages). Queued like `Pointer`.
    Wheel {
        dx: f32,
        dy: f32,
        x: f32,
        y: f32,
        modifiers: Modifiers,
        time_ms: f64,
    },
    Key {
        key: String,
        modifiers: Modifiers,
    },
    SetCamera {
        target: CameraTarget,
    },
    /// `None` picks 250–600 ms by distance (04 §13).
    FlyTo {
        target: CameraTarget,
        duration_ms: Option<u32>,
    },
    SetGestureHandling(GestureHandling),
    /// With reduced motion, camera changes are instant and drags have no inertia.
    SetReducedMotion(bool),
    Select {
        objects: Vec<ObjectRef>,
    },
    Deselect {
        objects: Vec<ObjectRef>,
    },
    ClearSelection,
    HoldResult {
        request: u32,
        outcome: HoldOutcome,
    },
    SetPricing {
        prices: Vec<CategoryPrice>,
    },
    SetFilter {
        categories: Vec<String>,
        price: Option<PriceRange>,
        accessible_only: bool,
    },
    /// A built-in theme with token overrides (05 §8). Each `SetTheme`
    /// replaces the whole theme; it persists across `LoadChart`.
    SetTheme {
        base: BuiltinTheme,
        tokens: Vec<ThemeToken>,
    },
    /// The baked glyph atlas's pixels (04 §12), decoded by the host: RGBA8,
    /// `text::ATLAS_SIZE`, rows from the top. Seat labels are drawn from the
    /// first frame after it; without it there are none.
    SetGlyphAtlas {
        rgba: Vec<u8>,
    },
    /// The host's answer to `Event::TextRequest` `id` (04 §12): `width` ×
    /// `height` bytes of coverage, rows from the top, drawn at `px` device
    /// pixels per em (at most the size asked for). Empty when the host
    /// cannot draw the string.
    SetText {
        id: u32,
        width: u32,
        height: u32,
        px: u32,
        pixels: Vec<u8>,
    },
    /// A built-in string the engine draws (`soldOut`, default "Sold out"),
    /// from the SDK's message catalogue. At most 64 bytes.
    SetMessage {
        key: String,
        text: String,
    },
    SetMode(Mode),
    /// Refused with `InvalidInput` while anything is selected or a request
    /// is outstanding.
    SetSessionMode(SessionMode),
    /// `max`: at most this many objects, 1 to `tuning::MAX_SELECTED` (the
    /// default). `min`: at least this many before checkout.
    SetRules {
        max: Option<u32>,
        min: Option<u32>,
        validators: Vec<String>,
        group_size: Option<u32>,
    },
    SetFloor {
        floor: String,
    },
    /// Frame time in milliseconds, from `requestAnimationFrame` or a display link.
    Tick {
        time_ms: f64,
    },
    ContextLost,
    ContextRestored,
}

impl Command {
    /// The state command a realtime server frame carries (08 §8.1):
    /// `SetSnapshot`, `ApplyDelta` or `SetPrivateHolds`. `None` for the frames
    /// the adapter handles itself and for unknown frame types, which are
    /// skipped (08 §8.6). Every `idx`, ignored area entries included, is
    /// checked against `objects`, the receiver's object count; the engine
    /// checks the rest. Before a chart (`None`) the area entries a DELTA or
    /// PRIVATE carries could never be checked, so such a frame is refused
    /// with `OutOfRange`; servers send none yet.
    pub fn from_frame(frame: &[u8], objects: Option<u32>) -> Result<Option<Self>, WireError> {
        let limit = cavea_core::limits::MAX_OBJECTS;
        let all = objects.map_or(limit, |n| n.min(limit));
        let frame = decode_server(frame, View::Public);
        let areas = match frame {
            Ok(ServerFrame::Delta(d)) => d.areas(all)?.next(),
            Ok(ServerFrame::Private(p)) => p.areas(all)?.next(),
            _ => None,
        };
        if objects.is_none() && areas.is_some() {
            return Err(WireError::OutOfRange);
        }
        Ok(Some(match frame {
            Ok(ServerFrame::Snapshot(s)) => {
                let mut state = vec![0; s.objects as usize];
                s.write_into(&mut state)?;
                Self::SetSnapshot { seq: s.seq, state }
            }
            Ok(ServerFrame::Delta(d)) => Self::ApplyDelta {
                from_seq: d.from_seq,
                to_seq: d.to_seq,
                changes: (d.changes(all)?)
                    .map(|c| StatusChange {
                        idx: c.idx,
                        status: c.code,
                    })
                    .collect(),
            },
            Ok(ServerFrame::Private(p)) => Self::SetPrivateHolds {
                objects: p.objects(all)?.collect(),
                expires_at_ms: p.expires_at_ms,
            },
            Ok(_) | Err(WireError::UnknownType) => return Ok(None),
            Err(e) => return Err(e),
        }))
    }
}

/// A floor, in chart order; its index is its id in `SetFloor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloorInfo {
    pub name: String,
    pub level: i32,
    pub seats: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionInfo {
    /// Empty for a section without a label.
    pub label: String,
    /// Index into `ChartInfo::floors`.
    pub floor: u32,
    pub seats: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryInfo {
    pub key: String,
    pub label: String,
    /// The chart's colour, RGBA.
    pub color: [u8; 4],
    /// Bookable objects in this category.
    pub objects: u32,
}

/// Chart metadata reported by `Ready`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartInfo {
    pub name: String,
    pub floors: Vec<FloorInfo>,
    pub sections: Vec<SectionInfo>,
    pub categories: Vec<CategoryInfo>,
    /// Bookable objects: seats, then tables booked whole, booths and areas.
    pub objects: u32,
    pub seats: u32,
}

/// One object as the host sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectInfo {
    pub label: String,
    pub category: String,
    pub section: String,
    pub row: String,
    pub seat: String,
    pub flags: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lod {
    Overview,
    CrossFade,
    Seats,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    InitStart,
    WasmReady,
    ChartFetched,
    ChartDecoded,
    GpuReady,
    FirstFrame,
    StateReady,
    Interactive,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameStats {
    pub median_ms: f32,
    pub p95_ms: f32,
    pub max_ms: f32,
    pub draw_calls: u32,
    pub visible_seats: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// The GPU backend failed.
    Gpu,
    /// The command is not implemented in this build.
    Unsupported,
    /// The command's data is invalid.
    InvalidInput,
    /// `LoadChart` bytes are not a valid CCF artefact.
    BadChart,
}

/// Output of the engine (04 §3).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Ready(ChartInfo),
    NeedFrame,
    HoldRequest {
        request: u32,
        labels: Vec<String>,
    },
    ReleaseRequest {
        request: u32,
        labels: Vec<String>,
    },
    /// `expires_at_ms`: when the buyer's holds expire (Unix ms), from the
    /// last non-empty `SetPrivateHolds`.
    SelectionChanged {
        selected: Vec<ObjectInfo>,
        expires_at_ms: Option<u64>,
    },
    SelectionInvalid {
        rule: String,
        objects: Vec<String>,
    },
    SelectionValid,
    /// `None` when the pointer leaves every object. The anchor is the
    /// object's centre in CSS pixels on the canvas.
    ObjectHover(Option<(ObjectInfo, [f32; 2])>),
    ObjectFocus(Option<(ObjectInfo, [f32; 2])>),
    SectionEnter {
        section: String,
    },
    /// A selected object was taken by someone else, or its hold was refused
    /// because it is not free; it is no longer selected.
    ObjectUnavailable {
        label: String,
    },
    /// The buyer's hold on these selected objects is gone (expired, or
    /// released elsewhere); they are no longer selected.
    HoldLost {
        labels: Vec<String>,
    },
    /// A delta did not follow the state shown (`seq`, if any): fetch a new
    /// snapshot. Emitted once until one arrives.
    Resync {
        seq: Option<u64>,
    },
    CameraChanged {
        centre: [f32; 2],
        zoom: f32,
        lod: Lod,
    },
    Announce {
        text: String,
    },
    /// Draw `text` in the CSS font family `font` at `weight` and `px` device
    /// pixels per em, white on transparent, and answer with
    /// `Command::SetText` (04 §12). Emitted while building labels; the host
    /// rasterises after the frame, never during it.
    TextRequest {
        id: u32,
        text: String,
        px: u16,
        weight: u16,
        font: String,
    },
    Metrics {
        marks: Vec<(Mark, f64)>,
        frames: FrameStats,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}
