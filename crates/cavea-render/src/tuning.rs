//! Every camera, gesture and resolution constant, in one place (04 §26).
//! Distances are CSS pixels and times milliseconds unless the name says otherwise.

// Camera (04 §13).

/// Padding around the chart when fitting it to the viewport.
pub const FIT_PADDING_PX: f64 = 24.0;
/// The minimum zoom as a fraction of the fit zoom.
pub const MIN_ZOOM_OF_FIT: f64 = 0.8;
/// The maximum zoom shows a default seat this large.
pub const MAX_SEAT_PX: f64 = 64.0;
/// In each axis, at least this share of the viewport shows the chart (or the
/// whole chart, if it is smaller on screen).
pub const MIN_VISIBLE_FRACTION: f64 = 1.0 / 3.0;
pub const FLY_MIN_MS: f64 = 250.0;
pub const FLY_MAX_MS: f64 = 600.0;
/// Fly duration added per unit of distance: one doubling of zoom, or one
/// viewport diagonal of travel at the closer of the two zooms.
pub const FLY_MS_PER_UNIT: f64 = 100.0;
/// Zoom factor of one step (double tap, two-finger tap).
pub const ZOOM_STEP: f64 = 2.0;

// Gestures (04 §14).

/// Movement that turns a touch or pen press into a drag.
pub const DRAG_SLOP_TOUCH_PX: f64 = 8.0;
/// Movement that turns a mouse press into a drag.
pub const DRAG_SLOP_MOUSE_PX: f64 = 4.0;
/// A touch or pen press held this long without moving is a long press.
pub const LONG_PRESS_MS: f64 = 500.0;
/// The second tap of a double tap goes down within this time of the first
/// tap's release, and within `DOUBLE_TAP_DISTANCE_PX` of it.
pub const DOUBLE_TAP_MS: f64 = 300.0;
pub const DOUBLE_TAP_DISTANCE_PX: f64 = 24.0;
/// A two-finger tap lifts its last finger within this time of the first going down.
pub const TWO_FINGER_TAP_MS: f64 = 300.0;
/// Inertia: velocity decays as `exp(-t / INERTIA_TIME_CONSTANT_MS)`.
pub const INERTIA_TIME_CONSTANT_MS: f64 = 325.0;
/// Inertia stops below this speed, in CSS pixels per second.
pub const INERTIA_STOP_PX_PER_S: f64 = 10.0;
/// Release velocity is measured over the drag's last movement within this window.
pub const VELOCITY_WINDOW_MS: f64 = 100.0;
/// Wheel events further apart than this start a new wheel stream.
pub const WHEEL_STREAM_GAP_MS: f64 = 300.0;
/// A stream that starts with a vertical-only delta at least this large, or a
/// non-zero multiple of `WHEEL_MAC_NOTCH_PX`, is a mouse wheel (zoom); any
/// other stream is a trackpad (pan). A heuristic: browsers do not say.
pub const WHEEL_NOTCH_MIN_PX: f64 = 50.0;
/// The delta of one mouse-wheel notch in Chromium and Safari on macOS.
pub const WHEEL_MAC_NOTCH_PX: f64 = 4.000_244_140_625;
/// Mouse-wheel delta that doubles or halves the zoom.
pub const WHEEL_ZOOM_PX_PER_DOUBLING: f64 = 300.0;
/// Trackpad-pinch delta (wheel with ctrl) that doubles or halves the zoom.
pub const PINCH_WHEEL_PX_PER_DOUBLING: f64 = 100.0;
/// Pointers tracked at once; further presses are ignored.
pub const MAX_POINTERS: usize = 10;
/// Input commands held for the next `Tick` before they are applied at once.
pub const MAX_QUEUED_INPUT: usize = 256;

// Resolution (04 §21).

/// Default `maxDpr` on touch devices and elsewhere.
pub const MAX_DPR_TOUCH: f32 = 2.0;
pub const MAX_DPR_OTHER: f32 = 3.0;
/// Longest side of the backing store, in device pixels.
pub const MAX_BACKING_PX: f32 = 4096.0;

// Hit testing and selection (04 §15, §16).

/// A tap hits a seat within this distance of its centre, or within the seat's
/// half size if that is larger; touch and pen, then mouse (and hover).
pub const HIT_TOLERANCE_TOUCH_PX: f64 = 12.0;
pub const HIT_TOLERANCE_MOUSE_PX: f64 = 4.0;
/// Seats smaller than this on screen are not selected by a touch or pen tap:
/// the tap zooms to `ZOOM_TO_SELECT_SEAT_PX` instead (zoom-to-select).
pub const SELECT_MIN_SEAT_TOUCH_PX: f64 = 18.0;
/// The same for a mouse click.
pub const SELECT_MIN_SEAT_MOUSE_PX: f64 = 5.0;
/// Zoom-to-select makes the tapped seat this large, centred on the tap.
pub const ZOOM_TO_SELECT_SEAT_PX: f64 = 32.0;
/// Most objects a buyer can select: the largest object limit of a hold token
/// for publishable keys (`02-domain-model.md` §5), and the default maximum.
pub const MAX_SELECTED: u32 = 100;

// Overlays (04 §7 pass 12).

/// Width of the hover and focus rings and of the background-coloured halo
/// between a ring and its seat (11 §3: 2 px, two-tone).
pub const RING_WIDTH_PX: f32 = 2.0;

// Level of detail (04 §10).

/// Seats at least this large on screen are drawn at the detail level.
pub const LOD_DETAIL_SEAT_PX: f64 = 14.0;

/// Most seat draw calls in one frame (04 §11); more visible ranges are merged
/// across the smallest gaps.
pub const MAX_SEAT_DRAW_CALLS: usize = 64;
