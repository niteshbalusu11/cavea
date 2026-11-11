//! Camera maths (04 §13): fit, zoom limits, clamping and fly-to interpolation.
//! Pure functions over plain numbers with no renderer types, so the module can
//! move to `cavea-core` (camera maths is listed there, 01 §2). Screen positions
//! are CSS pixels from the viewport's top-left corner, y down.

use crate::tuning::{
    FIT_PADDING_PX, FLY_MAX_MS, FLY_MIN_MS, FLY_MS_PER_UNIT, MAX_SEAT_PX, MIN_VISIBLE_FRACTION,
    MIN_ZOOM_OF_FIT,
};

/// An axis-aligned rectangle in chart units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: [f64; 2],
    pub max: [f64; 2],
}

/// The viewport size in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// The zoom range for a chart and viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub min_zoom: f64,
    pub max_zoom: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// The chart point at the viewport's centre.
    pub centre: [f64; 2],
    /// CSS pixels per chart unit.
    pub zoom: f64,
    /// Rotation as (cos, sin). Fixed at zero, `[1, 0]`, in v1: only
    /// [`Camera::matrix`] and the screen conversions honour it; clamping assumes it.
    pub rotation: [f64; 2],
}

/// The zoom at which `rect` fills `size` with `FIT_PADDING_PX` on every side.
pub fn fit_zoom(rect: Rect, size: Size) -> f64 {
    let room = |px: f64| (px - 2.0 * FIT_PADDING_PX).max(1.0);
    (room(size.width) / (rect.max[0] - rect.min[0]))
        .min(room(size.height) / (rect.max[1] - rect.min[1]))
}

impl Limits {
    /// Minimum `0.8 × fit`; maximum where a seat of `seat_size` chart units is
    /// `MAX_SEAT_PX` on screen, but never below the minimum.
    pub fn new(chart: Rect, seat_size: f64, size: Size) -> Self {
        let min_zoom = MIN_ZOOM_OF_FIT * fit_zoom(chart, size);
        Self {
            min_zoom,
            max_zoom: (MAX_SEAT_PX / seat_size).max(min_zoom),
        }
    }

    pub fn clamp(self, zoom: f64) -> f64 {
        zoom.max(self.min_zoom).min(self.max_zoom)
    }
}

impl Camera {
    /// The camera that shows `rect` centred at its fit zoom.
    pub fn fit(rect: Rect, size: Size) -> Self {
        Self {
            centre: [
                (rect.min[0] + rect.max[0]) / 2.0,
                (rect.min[1] + rect.max[1]) / 2.0,
            ],
            zoom: fit_zoom(rect, size),
            rotation: [1.0, 0.0],
        }
    }

    pub fn to_screen(&self, world: [f64; 2], size: Size) -> [f64; 2] {
        let [cos, sin] = self.rotation;
        let d = [world[0] - self.centre[0], world[1] - self.centre[1]];
        [
            (cos * d[0] - sin * d[1]) * self.zoom + size.width / 2.0,
            (sin * d[0] + cos * d[1]) * self.zoom + size.height / 2.0,
        ]
    }

    pub fn to_world(&self, screen: [f64; 2], size: Size) -> [f64; 2] {
        let [cos, sin] = self.rotation;
        let d = [
            (screen[0] - size.width / 2.0) / self.zoom,
            (screen[1] - size.height / 2.0) / self.zoom,
        ];
        [
            self.centre[0] + cos * d[0] + sin * d[1],
            self.centre[1] - sin * d[0] + cos * d[1],
        ]
    }

    /// Moves the chart by `delta` screen pixels.
    pub fn pan(&mut self, delta: [f64; 2]) {
        let [cos, sin] = self.rotation;
        let d = [delta[0] / self.zoom, delta[1] / self.zoom];
        self.centre[0] -= cos * d[0] + sin * d[1];
        self.centre[1] -= -sin * d[0] + cos * d[1];
    }

    /// Multiplies the zoom by `factor`, within `limits`, keeping the chart
    /// point under the screen point `at` where it is.
    pub fn zoom_about(&mut self, factor: f64, at: [f64; 2], size: Size, limits: Limits) {
        let anchor = self.to_world(at, size);
        self.zoom = limits.clamp(self.zoom * factor);
        let moved = self.to_screen(anchor, size);
        self.pan([at[0] - moved[0], at[1] - moved[1]]);
    }

    /// Brings the zoom within `limits` and the centre to where, in each axis, at
    /// least `MIN_VISIBLE_FRACTION` of the viewport (or the whole chart, if it is
    /// smaller on screen) shows the chart.
    pub fn clamp(&mut self, chart: Rect, size: Size, limits: Limits) {
        self.zoom = limits.clamp(self.zoom);
        for (axis, view) in [size.width, size.height].into_iter().enumerate() {
            let (Some(c), Some(lo), Some(hi)) = (
                self.centre.get_mut(axis),
                chart.min.get(axis),
                chart.max.get(axis),
            ) else {
                continue;
            };
            let shown = (view * MIN_VISIBLE_FRACTION).min((hi - lo) * self.zoom);
            let slack = (view / 2.0 - shown) / self.zoom;
            *c = c.max(lo - slack).min(hi + slack);
        }
    }

    /// The camera `t` of the way from `self` to `to`: the centre linearly, the
    /// zoom linearly in log space.
    pub fn lerp(&self, to: &Self, t: f64) -> Self {
        let mix = |a: f64, b: f64| a + (b - a) * t;
        Self {
            centre: [
                mix(self.centre[0], to.centre[0]),
                mix(self.centre[1], to.centre[1]),
            ],
            zoom: self.zoom * ((to.zoom / self.zoom).ln() * t).exp(),
            rotation: self.rotation,
        }
    }

    /// Chart units to clip space, column-major, for a viewport of `size`.
    pub fn matrix(&self, size: Size) -> [[f32; 3]; 3] {
        let [cos, sin] = self.rotation;
        let (sx, sy) = (2.0 * self.zoom / size.width, -2.0 * self.zoom / size.height);
        let col0 = [sx * cos, sy * sin];
        let col1 = [-sx * sin, sy * cos];
        let [cx, cy] = self.centre;
        let col2 = [
            -(col0[0] * cx + col1[0] * cy),
            -(col0[1] * cx + col1[1] * cy),
        ];
        let f = |v: f64| v as f32;
        [
            [f(col0[0]), f(col0[1]), 0.0],
            [f(col1[0]), f(col1[1]), 0.0],
            [f(col2[0]), f(col2[1]), 1.0],
        ]
    }
}

/// Fly-to duration (04 §13): `FLY_MIN_MS` plus `FLY_MS_PER_UNIT` per doubling
/// of zoom and per viewport diagonal of travel, at most `FLY_MAX_MS`.
pub fn fly_duration_ms(from: &Camera, to: &Camera, size: Size) -> f64 {
    let doublings = (to.zoom / from.zoom).ln().abs() / std::f64::consts::LN_2;
    let travel = length([to.centre[0] - from.centre[0], to.centre[1] - from.centre[1]])
        * from.zoom.min(to.zoom)
        / length([size.width, size.height]);
    (FLY_MIN_MS + FLY_MS_PER_UNIT * (doublings + travel)).min(FLY_MAX_MS)
}

/// The length of a vector. Not `f64::hypot`, which costs 2.6 KB of WASM and
/// guards against overflow that chart and screen sizes cannot reach.
pub fn length(v: [f64; 2]) -> f64 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

/// Cubic ease-out over `t` in `[0, 1]`.
pub fn ease_out(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}
