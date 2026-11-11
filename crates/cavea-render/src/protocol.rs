//! The engine protocol (`docs/04-renderer.md` §3, `docs/DECISIONS.md` D-21).
//! The engine is driven only by [`Command`]s and emits only [`Event`]s. Both are
//! plain owned data with no handles, references or closures, so the engine can
//! later run in a worker or behind FFI unchanged. A wire encoding is added when
//! the first such boundary exists.

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
pub struct PrivateHold {
    pub idx: u32,
    /// Unix time in milliseconds.
    pub expires_at_ms: u64,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Picker,
    Static,
    Spotlight,
    Manager,
}

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
    SetSnapshot {
        seq: u64,
        state: Vec<u8>,
    },
    ApplyDelta {
        from_seq: u64,
        to_seq: u64,
        changes: Vec<StatusChange>,
    },
    SetPrivateHolds {
        holds: Vec<PrivateHold>,
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
    SetTheme {
        tokens: Vec<ThemeToken>,
    },
    SetMode(Mode),
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    SelectionChanged {
        selected: Vec<ObjectInfo>,
    },
    SelectionInvalid {
        rule: String,
        objects: Vec<String>,
    },
    SelectionValid,
    /// `None` when the pointer leaves every object. Anchor in CSS pixels.
    ObjectHover(Option<(ObjectInfo, [f32; 2])>),
    ObjectFocus(Option<(ObjectInfo, [f32; 2])>),
    SectionEnter {
        section: String,
    },
    ObjectUnavailable {
        label: String,
    },
    CameraChanged {
        centre: [f32; 2],
        zoom: f32,
        lod: Lod,
    },
    Announce {
        text: String,
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
