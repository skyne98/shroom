//! Motion: every visible state change is followed by a short spring
//! animation, the same in the terminal and the window.
//!
//! State stays discrete and instant (logic and tests never wait for an
//! animation). The view derives each animated value from its target once per
//! frame with [`Motions::follow`], so a change of state *is* the start of its
//! animation, and a change mid-flight retargets the spring with its velocity
//! kept: interruptible, never a restart from zero.
//!
//! Springs are solved in closed form (mass 1), so any frame rate gives the same
//! motion: one 100 ms step lands exactly where ten 10 ms steps do. Tuned by
//! feel the way UIKit/SwiftUI are: `response` (period of the undamped spring,
//! seconds) and `damping` ratio (1 = fastest without overshoot, below 1
//! overshoots a little).
//!
//! `SHROOM_REDUCED_MOTION=1` (or `NO_MOTION`) snaps every value: motion is
//! never required to read the screen.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::layout::Run;

/// A spring, by feel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    /// Period of the undamped oscillation, seconds: smaller is snappier.
    pub response: f64,
    /// 1 = critically damped (no overshoot); 0.7 overshoots by about 5 %.
    pub damping: f64,
}

impl Spring {
    /// Scrolling and anything that moves content: fast, no overshoot.
    pub const GLIDE: Spring = Spring {
        response: 0.16,
        damping: 1.0,
    };
    /// Things appearing (rows, notices, highlights): fade in quickly.
    pub const FADE: Spring = Spring {
        response: 0.14,
        damping: 1.0,
    };
    /// Popups opening: a small overshoot that reads as "it popped".
    pub const POP: Spring = Spring {
        response: 0.2,
        damping: 0.72,
    };
    /// Numbers counting to a new value (percentages, totals).
    pub const COUNT: Spring = Spring {
        response: 0.3,
        damping: 1.0,
    };

    // Material 3 Expressive's motion scheme (Compose `MotionScheme.expressive()`,
    // `ExpressiveMotionTokens`). Compose springs are (stiffness, damping ratio)
    // at mass 1, so `response = TAU / sqrt(stiffness)` (see `from_stiffness`).
    /// Expressive fast spatial: stiffness 800, damping 0.6 (moves and shapes
    /// of small components; overshoots about 9.5 %, by spec).
    pub const FAST_SPATIAL: Spring = Spring {
        response: 0.222_144_146_907_918_3,
        damping: 0.6,
    };
    /// Expressive fast effects: stiffness 3800, damping 1 (colour, opacity).
    pub const FAST_EFFECTS: Spring = Spring {
        response: 0.101_926_724_975_859_9,
        damping: 1.0,
    };
    /// Expressive default effects: stiffness 1600, damping 1 (colour, opacity,
    /// and the shape morph of buttons, which must not bounce).
    pub const DEFAULT_EFFECTS: Spring = Spring {
        response: 0.157_079_632_679_489_66,
        damping: 1.0,
    };

    /// A Compose spring (mass 1): `stiffness` in 1/s², `damping` ratio.
    pub fn from_stiffness(stiffness: f64, damping: f64) -> Spring {
        Spring {
            response: std::f64::consts::TAU / stiffness.sqrt(),
            damping,
        }
    }

    fn omega(self) -> f64 {
        std::f64::consts::TAU / self.response.max(1e-3)
    }

    /// Position and velocity after `dt` seconds, from offset `y` (value minus
    /// target) moving at `v`. Exact for every damping ratio.
    pub fn solve(self, y: f64, v: f64, dt: f64) -> (f64, f64) {
        let w = self.omega();
        let z = self.damping.max(0.0);
        if (z - 1.0).abs() < 1e-6 {
            let e = (-w * dt).exp();
            let c = v + w * y;
            return ((y + c * dt) * e, (v - w * c * dt) * e);
        }
        if z < 1.0 {
            let wd = w * (1.0 - z * z).sqrt();
            let e = (-z * w * dt).exp();
            let (sin, cos) = (wd * dt).sin_cos();
            let a = (v + z * w * y) / wd;
            let b = (z * w * v + w * w * y) / wd;
            return (e * (y * cos + a * sin), e * (v * cos - b * sin));
        }
        let root = (z * z - 1.0).sqrt();
        let (r1, r2) = (-w * (z - root), -w * (z + root));
        let c2 = (v - r1 * y) / (r2 - r1);
        let c1 = y - c2;
        let (e1, e2) = ((r1 * dt).exp(), (r2 * dt).exp());
        (c1 * e1 + c2 * e2, r1 * c1 * e1 + r2 * c2 * e2)
    }
}

// ----------------------------------------------------------- zeron curves

/// A CSS `cubic-bezier(x1, y1, x2, y2)` timing function, ported from zeron's
/// motion kit: Newton iteration with a bisection fallback (UnitBezier).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

impl CubicBezier {
    pub const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Self { x1, y1, x2, y2 }
    }

    fn coefficients(a: f64, b: f64) -> (f64, f64, f64) {
        let c = 3.0 * a;
        let bb = 3.0 * (b - a) - c;
        (1.0 - c - bb, bb, c)
    }

    fn sample(a: f64, b: f64, t: f64) -> f64 {
        let (aa, bb, cc) = Self::coefficients(a, b);
        ((aa * t + bb) * t + cc) * t
    }

    fn solve_t_for_x(&self, x: f64) -> f64 {
        let (a, b, c) = Self::coefficients(self.x1, self.x2);
        let mut t = x;
        for _ in 0..8 {
            let err = Self::sample(self.x1, self.x2, t) - x;
            if err.abs() < 1e-9 {
                return t;
            }
            let d = (3.0 * a * t + 2.0 * b) * t + c;
            if d.abs() < 1e-9 {
                break;
            }
            t -= err / d;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..48 {
            let mid = (lo + hi) / 2.0;
            if Self::sample(self.x1, self.x2, mid) < x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (lo + hi) / 2.0
    }

    /// Eased output for progress `x` in 0..1 (clamped).
    pub fn eval(&self, x: f64) -> f64 {
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        Self::sample(self.y1, self.y2, self.solve_t_for_x(x)).clamp(0.0, 1.0)
    }
}

/// zeron's entrance curve, `cubic-bezier(0.16, 1, 0.3, 1)`.
pub const EASE_OUT_EXPO: CubicBezier = CubicBezier::new(0.16, 1.0, 0.3, 1.0);
/// CSS `ease-out`: sizes.
pub const EASE_OUT: CubicBezier = CubicBezier::new(0.0, 0.0, 0.58, 1.0);
/// CSS `ease`: quick fades, menus, dialogs.
pub const EASE: CubicBezier = CubicBezier::new(0.25, 0.1, 0.25, 1.0);
/// Tailwind's default transition curve: hover fades.
pub const EASE_TAILWIND: CubicBezier = CubicBezier::new(0.4, 0.0, 0.2, 1.0);
/// Constant speed (Compose `LinearEasing`).
pub const LINEAR: CubicBezier = CubicBezier::new(0.0, 0.0, 1.0, 1.0);
/// Material's standard curve (Compose `FastOutSlowInEasing`).
pub const FAST_OUT_SLOW_IN: CubicBezier = CubicBezier::new(0.4, 0.0, 0.2, 1.0);
/// Material's emphasized curve (material-web `easing-emphasized`).
pub const EMPHASIZED: CubicBezier = CubicBezier::new(0.2, 0.0, 0.0, 1.0);
/// Compose's outgoing elevation curve.
pub const ELEVATION_OUT_CURVE: CubicBezier = CubicBezier::new(0.4, 0.0, 0.6, 1.0);

/// A timed eased transition (zeron's `MotionSpec`): a duration and a curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ease {
    pub ms: u64,
    pub curve: CubicBezier,
}

impl Ease {
    pub const fn new(ms: u64, curve: CubicBezier) -> Self {
        Self { ms, curve }
    }

    fn secs(self) -> f64 {
        self.ms as f64 / 1000.0
    }

    // zeron's catalog.
    /// Entrances: rows, the welcome, a session switch.
    pub const FADE_IN: Ease = Ease::new(500, EASE_OUT_EXPO);
    /// Small things appearing (notices).
    pub const FADE_QUICK: Ease = Ease::new(150, EASE);
    /// Popovers opening.
    pub const MENU_IN: Ease = Ease::new(140, EASE);
    /// Popovers closing.
    pub const MENU_OUT: Ease = Ease::new(100, EASE);
    /// Dialogs opening and closing.
    pub const DIALOG_IN: Ease = Ease::new(180, EASE);
    /// Sizes and sliders (panes, the prompt box, the effort thumb).
    pub const RESIZE: Ease = Ease::new(200, EASE_OUT);
    /// Hover and highlight plates.
    pub const HOVER_FADE: Ease = Ease::new(150, EASE_TAILWIND);

    // Material 3 interaction states (Compose `Ripple` and `Elevation`,
    // material-web's focus ring). Durations and curves are the spec's.
    /// A state layer coming or going: hover in and out, focus out.
    pub const STATE_LAYER: Ease = Ease::new(15, LINEAR);
    /// The focus and drag state layers coming in.
    pub const STATE_LAYER_IN: Ease = Ease::new(45, LINEAR);
    /// The drag state layer going.
    pub const STATE_LAYER_DRAG_OUT: Ease = Ease::new(150, LINEAR);
    /// Elevation rising.
    pub const ELEVATION_IN: Ease = Ease::new(120, FAST_OUT_SLOW_IN);
    /// Elevation falling.
    pub const ELEVATION_OUT: Ease = Ease::new(150, ELEVATION_OUT_CURVE);
    /// Elevation falling when the pointer leaves.
    pub const ELEVATION_OUT_HOVER: Ease = Ease::new(120, ELEVATION_OUT_CURVE);
    /// A press ripple fading in.
    pub const RIPPLE_IN: Ease = Ease::new(75, LINEAR);
    /// A press ripple growing to cover its component.
    pub const RIPPLE_GROW: Ease = Ease::new(225, FAST_OUT_SLOW_IN);
    /// A press ripple's centre moving to the component's.
    pub const RIPPLE_CENTER: Ease = Ease::new(225, LINEAR);
    /// A released ripple fading out.
    pub const RIPPLE_OUT: Ease = Ease::new(150, LINEAR);
    /// The focus ring growing to its active width (first quarter of 600 ms).
    pub const FOCUS_RING_GROW: Ease = Ease::new(150, EMPHASIZED);
    /// The focus ring settling to its width (the rest of the 600 ms).
    pub const FOCUS_RING_SHRINK: Ease = Ease::new(450, EMPHASIZED);
}

/// An eased transition in flight: from where it started, how far along.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tween {
    ease: Ease,
    from: f64,
    elapsed: f64,
}

/// Offset (in the value's units) and speed (units/s) below which a spring is
/// at rest: a 200th of a row or of full opacity is invisible.
const REST: f64 = 0.005;
const REST_SPEED: f64 = 0.05;
/// Longest step one frame may take: a frame after a stall continues the
/// motion instead of jumping to its end.
const MAX_STEP: f64 = 1.0 / 30.0;

/// One animated value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub value: f64,
    pub velocity: f64,
    pub target: f64,
    pub spring: Spring,
    /// Set: an eased transition (zeron's curves) instead of the spring.
    tween: Option<Tween>,
}

impl Motion {
    pub fn at(value: f64, spring: Spring) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
            spring,
            tween: None,
        }
    }

    pub fn step(&mut self, dt: f64) {
        if let Some(tween) = &mut self.tween {
            tween.elapsed += dt;
            let t = tween.elapsed / tween.ease.secs().max(1e-6);
            if t >= 1.0 {
                self.value = self.target;
                self.velocity = 0.0;
                self.tween = None;
            } else {
                self.value = tween.from + (self.target - tween.from) * tween.ease.curve.eval(t);
            }
            return;
        }
        if self.resting() {
            self.value = self.target;
            self.velocity = 0.0;
            return;
        }
        let (y, v) = self
            .spring
            .solve(self.value - self.target, self.velocity, dt);
        self.value = self.target + y;
        self.velocity = v;
        if self.resting() {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }

    pub fn resting(&self) -> bool {
        if self.tween.is_some() {
            return false;
        }
        (self.value - self.target).abs() < REST && self.velocity.abs() < REST_SPEED
    }
}

/// What a motion animates; one value per key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// Transcript scroll, rows from the bottom.
    Scroll,
    /// Opacity of the whole transcript (session switch, relayout).
    Content,
    /// Opacity of one transcript row, by absolute index.
    Row(usize),
    /// Popup openness: 0 closed, 1 open.
    Popup,
    /// Opacity of the highlighted popup item.
    Pick,
    /// Opacity of the status notice.
    Notice,
    /// Prompt box height, rows.
    Input,
    /// Context meter, share of the window.
    Context,
    /// Subagent dock height, rows.
    Dock,
    /// An open list's scroll position, rows.
    List,
    /// How open a scrollbar is (0 collapsed, 1 hovered), by bar id.
    Bar(usize),
    /// The window's sidebar: 0 hidden, 1 shown.
    Sidebar,
    /// The model and effort menu: opacity 0 closed, 1 open; and its scale.
    EffortPanel,
    EffortScale,
    /// A pointer-hover highlight (0 off, 1 on), by the client's id.
    Hover(u64),
    /// One part of an interactive component's motion, by the client's id.
    Control(u64, Part),
}

/// The animated parts of an interactive component (see [`Controls`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Hovered,
    Focused,
    Pressed,
    Checked,
    CheckedShape,
    Elevation,
    Outline,
    /// The corner radii: top and bottom (see [`Shapes`]).
    CornerTop,
    CornerBottom,
    RingGrow,
    RingShrink,
    RippleIn(u32),
    RippleGrow(u32),
    RippleCenter(u32),
    RippleOut(u32),
    /// A text field's label: 0 resting in the field, 1 floated above the
    /// text (and shrunk to the small type).
    Label,
    /// A text field's focus colours (label and active indicator): 0
    /// unfocused, 1 focused.
    FieldFocus,
    /// A text field's active indicator: 0 at its resting height, 1 at the
    /// focused one.
    Indicator,
    /// A text field's placeholder opacity.
    Placeholder,
}

/// Every running motion. Protocol, once per frame: [`Motions::tick`] first,
/// then `follow`/`start`/`nudge` while the frame is built. A change made
/// that way starts at the frame it is first drawn in, however long the screen
/// sat idle before it.
pub struct Motions {
    motions: HashMap<Key, Motion>,
    last: Option<Instant>,
    /// Snap instead of animating.
    pub reduced: bool,
}

impl Default for Motions {
    fn default() -> Self {
        Self::new(reduced_from_env())
    }
}

/// `SHROOM_REDUCED_MOTION` set to anything but `0`, or `NO_MOTION` set.
pub fn reduced_from_env() -> bool {
    let set = |name: &str| std::env::var(name).is_ok_and(|value| value != "0" && !value.is_empty());
    set("SHROOM_REDUCED_MOTION") || set("NO_MOTION")
}

impl Motions {
    pub fn new(reduced: bool) -> Self {
        Self {
            motions: HashMap::new(),
            last: None,
            reduced,
        }
    }

    /// Advance everything to `now`. The first frame after a rest starts from
    /// zero elapsed time, so an animation never begins half finished.
    pub fn tick(&mut self, now: Instant) {
        let dt = match self.last {
            Some(last) if self.active() => now.saturating_duration_since(last).as_secs_f64(),
            _ => 0.0,
        };
        self.last = Some(now);
        let dt = dt.min(MAX_STEP);
        for motion in self.motions.values_mut() {
            motion.step(dt);
        }
        // Rows and other one-shots are dropped once they rest at their end.
        self.motions
            .retain(|key, motion| !(matches!(key, Key::Row(_)) && motion.resting()));
    }

    /// The animated value for `key` heading to `target`; the first call
    /// starts at the target (nothing to animate yet).
    pub fn follow(&mut self, key: Key, target: f64, spring: Spring) -> f64 {
        let reduced = self.reduced;
        let motion = self
            .motions
            .entry(key)
            .or_insert_with(|| Motion::at(target, spring));
        motion.target = target;
        motion.spring = spring;
        if reduced {
            *motion = Motion::at(target, spring);
        }
        motion.value
    }

    /// Start `key` at `from` heading to `to` (an entrance).
    pub fn start(&mut self, key: Key, from: f64, to: f64, spring: Spring) {
        let value = if self.reduced { to } else { from };
        self.motions.insert(
            key,
            Motion {
                value,
                velocity: 0.0,
                target: to,
                spring,
                tween: None,
            },
        );
    }

    /// Like [`follow`](Self::follow), on one of zeron's eased curves: a new
    /// target starts a fresh transition from where the value is now, so it
    /// never jumps.
    pub fn ease(&mut self, key: Key, target: f64, ease: Ease) -> f64 {
        let reduced = self.reduced;
        let motion = self
            .motions
            .entry(key)
            .or_insert_with(|| Motion::at(target, Spring::FADE));
        if reduced {
            *motion = Motion::at(target, Spring::FADE);
        } else if motion.target != target {
            motion.tween = Some(Tween {
                ease,
                from: motion.value,
                elapsed: 0.0,
            });
            motion.target = target;
            motion.velocity = 0.0;
        }
        motion.value
    }

    /// Start `key` at `from` easing to `to` (an entrance, zeron's curves).
    pub fn enter(&mut self, key: Key, from: f64, to: f64, ease: Ease) {
        let mut motion = Motion::at(to, Spring::FADE);
        if !self.reduced {
            motion.value = from;
            motion.tween = Some(Tween {
                ease,
                from,
                elapsed: 0.0,
            });
        }
        self.motions.insert(key, motion);
    }

    /// Shift `key`'s current value by `by`, target unchanged: content that
    /// jumped stays put on screen, then glides to where it now belongs.
    pub fn nudge(&mut self, key: Key, by: f64) {
        if self.reduced {
            return;
        }
        if let Some(motion) = self.motions.get_mut(&key) {
            motion.value += by;
        }
    }

    /// Jump `key` to `value`, at rest.
    pub fn snap(&mut self, key: Key, value: f64) {
        if let Some(motion) = self.motions.get_mut(&key) {
            *motion = Motion::at(value, motion.spring);
        }
    }

    pub fn value(&self, key: &Key) -> Option<f64> {
        self.motions.get(key).map(|motion| motion.value)
    }

    /// `key` is at its target and still (or not animated at all).
    pub fn settled(&self, key: &Key) -> bool {
        self.motions.get(key).is_none_or(Motion::resting)
    }

    /// Drop `key`'s motion.
    pub fn forget(&mut self, key: &Key) {
        self.motions.remove(key);
    }

    /// Drop every motion of component `id`.
    pub fn forget_control(&mut self, id: u64) {
        self.motions
            .retain(|key, _| !matches!(key, Key::Control(of, _) if *of == id));
    }

    /// Clear all per-row motions (a different transcript is shown).
    pub fn forget_rows(&mut self) {
        self.motions.retain(|key, _| !matches!(key, Key::Row(_)));
    }

    /// Something is still moving: the client should draw at full frame rate.
    pub fn active(&self) -> bool {
        self.motions.values().any(|motion| !motion.resting())
    }
}

// ------------------------------------------------------------ components

/// What happened to an interactive component (the client reports these).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ControlEvent {
    /// The pointer entered (true) or left it.
    Hover(bool),
    /// A press at `at` (from its top-left corner) on a component of `size`,
    /// both in the client's pixels. A keyboard press is at the centre.
    Press { at: (f32, f32), size: (f32, f32) },
    /// The press ended (released or cancelled).
    Release,
}

/// What a component is this frame (the client knows it while drawing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlSpec {
    pub enabled: bool,
    /// Toggled on (toggle buttons, selected items).
    pub checked: bool,
    /// Keyboard focus is on it (focus-visible, not a click's focus).
    pub focused: bool,
    /// Elevation level at rest, hovered, focused and pressed.
    pub elevation: [f32; 4],
    /// Outline width now, pixels (0: none).
    pub outline: f32,
    /// How the shape morphs (pressed, checked).
    pub shape: Spring,
    /// How the colours move to checked (Compose: default effects; menus:
    /// fast effects).
    pub colors: Spring,
    /// Corner radii by state, morphing on `shape` (None: fixed, the client's).
    pub shapes: Option<Shapes>,
}

/// Corner radii, px (unzoomed): the top two and the bottom two. Expressive
/// shapes differ only between top and bottom (segmented lists, menus).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Corners {
    pub top: f32,
    pub bottom: f32,
}

impl Corners {
    pub const fn all(radius: f32) -> Self {
        Self {
            top: radius,
            bottom: radius,
        }
    }

    pub const fn new(top: f32, bottom: f32) -> Self {
        Self { top, bottom }
    }
}

/// A component's shape in each state (Compose's `ListItemShapes`,
/// `FilterChipDefaults.shapes`, `MenuGroupShapes`): the target is the
/// first that applies of pressed, checked, focused, hovered, inactive, rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shapes {
    pub rest: Corners,
    pub hovered: Corners,
    pub focused: Corners,
    pub pressed: Corners,
    pub checked: Corners,
    /// Once hovered and then left (Compose's menu group `inactiveShape`).
    pub inactive: Corners,
}

impl Shapes {
    /// The same shape in every state.
    pub const fn fixed(corners: Corners) -> Self {
        Self {
            rest: corners,
            hovered: corners,
            focused: corners,
            pressed: corners,
            checked: corners,
            inactive: corners,
        }
    }
}

impl Default for ControlSpec {
    fn default() -> Self {
        Self {
            enabled: true,
            checked: false,
            focused: false,
            elevation: [0.0; 4],
            outline: 0.0,
            shape: Spring::DEFAULT_EFFECTS,
            colors: Spring::DEFAULT_EFFECTS,
            shapes: None,
        }
    }
}

/// One press ripple as drawn: where it started, on what size, and how far
/// each of its motions has come (the client turns that into a circle).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RippleLook {
    pub at: (f32, f32),
    pub size: (f32, f32),
    /// Opacity, 0..1 of the pressed state layer.
    pub alpha: f32,
    /// 0..1 from the start radius to covering the component.
    pub grow: f32,
    /// 0..1 from the press point to the centre.
    pub center: f32,
}

/// A component's animated look this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlLook {
    /// 0..1 each: how much of the hover and focus state layers shows.
    pub hovered: f32,
    pub focused: f32,
    /// 0..1 toward the pressed shape.
    pub pressed: f32,
    /// 0..1 toward the checked colours, and toward the checked shape.
    pub checked: f32,
    pub checked_shape: f32,
    /// Elevation level (fractional while it moves).
    pub elevation: f32,
    /// Outline width, pixels.
    pub outline: f32,
    /// Corner radii now (with `ControlSpec::shapes`), px unzoomed.
    pub corners: Corners,
    /// The pointer is over it (not eased: for what depends on it, such as
    /// a menu's open groups).
    pub over: bool,
    /// Focus ring width in pixels while keyboard focus is on it.
    pub ring: Option<f32>,
    pub ripples: Vec<RippleLook>,
}

fn reduced_now(motions: &Motions) -> bool {
    motions.reduced
}

/// The hover state layer's fade, in and out (see `Controls::look`).
pub const HOVER_FADE: Spring = Spring::DEFAULT_EFFECTS;

/// Compose's `ClosedScaleTarget`: a menu opens from, and closes to, 80 %.
pub const MENU_CLOSED_SCALE: f64 = 0.8;

/// Width of the focus ring at its peak and at rest (material-web).
pub const RING_ACTIVE: f32 = 8.0;
pub const RING_WIDTH: f32 = 3.0;

#[derive(Clone, Copy, Debug)]
struct Ripple {
    slot: u32,
    at: (f32, f32),
    size: (f32, f32),
    started: bool,
    released: bool,
    fading: bool,
}

#[derive(Clone, Debug, Default)]
struct Control {
    hovered: bool,
    pressed: bool,
    ring: bool,
    /// The focus ring has started settling to its resting width.
    ring_settling: bool,
    /// The elevation's current target comes from hovering.
    raised_by_hover: bool,
    /// Hovered at least once (Compose's `hasBeenHovered`).
    been_hovered: bool,
    /// A text field's input phase last frame, and the spring its
    /// placeholder fades on since the last change of phase.
    phase: Option<Phase>,
    placeholder: Option<Spring>,
    ripples: Vec<Ripple>,
    next: u32,
}

/// What a text field is this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FieldSpec {
    /// It holds keyboard focus (typing lands in it).
    pub focused: bool,
    /// It has text.
    pub populated: bool,
}

/// A text field's animated look (Compose `TextFieldImpl`'s transition).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FieldLook {
    /// 0..1 of the hover state layer (and the hover indicator colour).
    pub hovered: f32,
    /// `labelProgress`: 0 the label rests in the field, 1 it has floated.
    /// Fast spatial: it overshoots a little, as in Compose.
    pub label: f32,
    /// 0..1 toward the focused label and indicator colours.
    pub focus: f32,
    /// 0..1 from the resting indicator height to the focused one (fast
    /// spatial, so it can overshoot too).
    pub indicator: f32,
    /// The placeholder's opacity.
    pub placeholder: f32,
}

/// Compose's `InputPhase`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Focused,
    UnfocusedEmpty,
    UnfocusedNotEmpty,
}

/// Material 3 interaction states of every component the client draws:
/// state layers, the press ripple, the shape morph, elevation and the focus
/// ring, each on the spec's curve. Protocol: `event` as input arrives,
/// `frame` right after [`Motions::tick`], then `look` while drawing.
#[derive(Debug, Default)]
pub struct Controls {
    controls: HashMap<u64, Control>,
}

impl Controls {
    /// Component `id` is gone (a closed menu's): it starts afresh.
    pub fn forget(&mut self, motions: &mut Motions, id: u64) {
        self.controls.remove(&id);
        motions.forget_control(id);
    }

    pub fn event(&mut self, id: u64, event: ControlEvent) {
        let control = self.controls.entry(id).or_default();
        match event {
            ControlEvent::Hover(on) => {
                control.hovered = on;
                control.been_hovered |= on;
            }
            ControlEvent::Press { at, size } => {
                control.pressed = true;
                let slot = control.next;
                control.next = control.next.wrapping_add(1);
                control.ripples.push(Ripple {
                    slot,
                    at,
                    size,
                    started: false,
                    released: false,
                    fading: false,
                });
            }
            ControlEvent::Release => {
                control.pressed = false;
                for ripple in &mut control.ripples {
                    ripple.released = true;
                }
            }
        }
    }

    /// Something waits for [`frame`](Self::frame) to start its next phase
    /// (a fresh press, a released ripple, a focus ring done growing), so the
    /// client must keep drawing frames even if every motion is at rest.
    pub fn busy(&self) -> bool {
        self.controls.values().any(|control| {
            control.ring && !control.ring_settling
                || control
                    .ripples
                    .iter()
                    .any(|ripple| !ripple.started || ripple.released && !ripple.fading)
        })
    }

    /// Start what input began since the last frame, and move ripples and
    /// focus rings on to their next phase.
    pub fn frame(&mut self, motions: &mut Motions) {
        for (&id, control) in &mut self.controls {
            let key = |part| Key::Control(id, part);
            for ripple in &mut control.ripples {
                let slot = ripple.slot;
                if !ripple.started {
                    ripple.started = true;
                    motions.enter(key(Part::RippleIn(slot)), 0.0, 1.0, Ease::RIPPLE_IN);
                    motions.enter(key(Part::RippleGrow(slot)), 0.0, 1.0, Ease::RIPPLE_GROW);
                    motions.enter(key(Part::RippleCenter(slot)), 0.0, 1.0, Ease::RIPPLE_CENTER);
                } else if ripple.released
                    && !ripple.fading
                    && [
                        Part::RippleIn(slot),
                        Part::RippleGrow(slot),
                        Part::RippleCenter(slot),
                    ]
                    .iter()
                    .all(|part| motions.settled(&key(*part)))
                {
                    // Out only once it has fully grown, however short the press.
                    ripple.fading = true;
                    motions.enter(key(Part::RippleOut(slot)), 1.0, 0.0, Ease::RIPPLE_OUT);
                }
            }
            control.ripples.retain(|ripple| {
                let slot = ripple.slot;
                let gone = ripple.fading && motions.settled(&key(Part::RippleOut(slot)));
                if gone {
                    for part in [
                        Part::RippleIn(slot),
                        Part::RippleGrow(slot),
                        Part::RippleCenter(slot),
                        Part::RippleOut(slot),
                    ] {
                        motions.forget(&key(part));
                    }
                }
                !gone
            });
            if control.ring
                && motions.settled(&key(Part::RingGrow))
                && motions.value(&key(Part::RingShrink)).is_none()
            {
                control.ring_settling = true;
                motions.enter(key(Part::RingShrink), 0.0, 1.0, Ease::FOCUS_RING_SHRINK);
            }
        }
    }

    /// Text field `id`'s look now (Compose `TextFieldImpl`: `labelProgress`,
    /// `placeholderOpacity`, `labelTextStyleColor`; `IndicatorLineNode`).
    /// Its hover comes through [`event`](Self::event) like any component's.
    pub fn field(&mut self, motions: &mut Motions, id: u64, spec: FieldSpec) -> FieldLook {
        let control = self.controls.entry(id).or_default();
        let key = |part| Key::Control(id, part);
        let on = |b: bool| f64::from(u8::from(b));
        let phase = match (spec.focused, spec.populated) {
            (true, _) => Phase::Focused,
            (false, false) => Phase::UnfocusedEmpty,
            (false, true) => Phase::UnfocusedNotEmpty,
        };
        // placeholderOpacity's transitionSpec: slow effects into focus from
        // empty, and from not-empty to empty; fast effects otherwise. The
        // expressive slow effects spring (stiffness 800) takes about 137 ms
        // to 90 %, past this app's 120 ms rule, so its place is taken by
        // default effects, the next faster effects spring of the scheme.
        if control.phase != Some(phase) {
            let slow = matches!(
                (control.phase, phase),
                (Some(Phase::UnfocusedEmpty), Phase::Focused)
                    | (Some(Phase::UnfocusedNotEmpty), Phase::UnfocusedEmpty)
            );
            control.placeholder = Some(if slow {
                Spring::DEFAULT_EFFECTS
            } else {
                Spring::FAST_EFFECTS
            });
            control.phase = Some(phase);
        }
        let hovered = motions.follow(key(Part::Hovered), on(control.hovered), HOVER_FADE);
        // Focused and UnfocusedNotEmpty float the label; UnfocusedEmpty
        // rests it (showExpandedLabel).
        let floated = phase != Phase::UnfocusedEmpty;
        let label = motions.follow(key(Part::Label), on(floated), Spring::FAST_SPATIAL);
        let focus = motions.follow(
            key(Part::FieldFocus),
            on(spec.focused),
            Spring::FAST_EFFECTS,
        );
        let indicator =
            motions.follow(key(Part::Indicator), on(spec.focused), Spring::FAST_SPATIAL);
        let placeholder = motions.follow(
            key(Part::Placeholder),
            on(phase == Phase::Focused),
            control.placeholder.unwrap_or(Spring::FAST_EFFECTS),
        );
        FieldLook {
            hovered: hovered.clamp(0.0, 1.0) as f32,
            label: label as f32,
            focus: focus.clamp(0.0, 1.0) as f32,
            indicator: indicator.max(0.0) as f32,
            placeholder: placeholder.clamp(0.0, 1.0) as f32,
        }
    }

    /// Component `id`'s look now, as `spec` says it is.
    pub fn look(&mut self, motions: &mut Motions, id: u64, spec: ControlSpec) -> ControlLook {
        let control = self.controls.entry(id).or_default();
        let key = |part| Key::Control(id, part);
        let on = |b: bool| f64::from(u8::from(b));
        let enabled = spec.enabled;
        let hovered = enabled && control.hovered;
        let pressed = enabled && control.pressed;
        let focused = enabled && spec.focused;

        // The guide: "Hover states appear and disappear using a low-emphasis
        // animated fade" (m3.material.io, Applying states). Compose's ripple
        // tweens it over 15 ms, which reads as a cut; the motion scheme's
        // default effects spring is the spec's animated fade.
        let hover = motions.follow(key(Part::Hovered), on(hovered), HOVER_FADE);
        let focus_ease = if focused {
            Ease::STATE_LAYER_IN
        } else {
            Ease::STATE_LAYER
        };
        let focus = motions.ease(key(Part::Focused), on(focused), focus_ease);
        let press = motions.follow(key(Part::Pressed), on(pressed), spec.shape);
        let checked = motions.follow(key(Part::Checked), on(spec.checked), spec.colors);
        let corners = spec.shapes.map_or_else(Corners::default, |shapes| {
            let target = if pressed {
                shapes.pressed
            } else if spec.checked {
                shapes.checked
            } else if focused {
                shapes.focused
            } else if hovered {
                shapes.hovered
            } else if control.been_hovered {
                shapes.inactive
            } else {
                shapes.rest
            };
            // A spring per side keeps its momentum through a reversal, as
            // Compose's `rememberAnimatedShape`.
            let top = motions.follow(key(Part::CornerTop), f64::from(target.top), spec.shape);
            let bottom = motions.follow(
                key(Part::CornerBottom),
                f64::from(target.bottom),
                spec.shape,
            );
            Corners::new(top.max(0.0) as f32, bottom.max(0.0) as f32)
        });
        let checked_shape = motions.follow(key(Part::CheckedShape), on(spec.checked), spec.shape);

        let [rest, hover_level, focus_level, press_level] = spec.elevation;
        let target = if !enabled {
            0.0
        } else if pressed {
            press_level
        } else if focused {
            focus_level
        } else if hovered {
            hover_level
        } else {
            rest
        };
        // Compose picks the curve by direction, and by what the level is
        // leaving: a hover's lift drops faster than a press's.
        let now = motions
            .value(&key(Part::Elevation))
            .unwrap_or(f64::from(target));
        let ease = if f64::from(target) >= now {
            Ease::ELEVATION_IN
        } else if control.raised_by_hover {
            Ease::ELEVATION_OUT_HOVER
        } else {
            Ease::ELEVATION_OUT
        };
        let elevation = motions.ease(key(Part::Elevation), f64::from(target), ease);
        control.raised_by_hover = hovered && !pressed && !focused;
        let outline = motions.follow(
            key(Part::Outline),
            f64::from(spec.outline),
            Spring::FAST_SPATIAL,
        );

        // The focus ring: grows to its active width, then settles (frame()
        // starts the second phase once the first is done).
        if focused && !control.ring {
            control.ring = true;
            control.ring_settling = reduced_now(motions);
            motions.forget(&key(Part::RingShrink));
            motions.enter(key(Part::RingGrow), 0.0, 1.0, Ease::FOCUS_RING_GROW);
        } else if !focused && control.ring {
            control.ring = false;
            control.ring_settling = false;
            motions.forget(&key(Part::RingGrow));
            motions.forget(&key(Part::RingShrink));
        }
        let reduced = motions.reduced;
        let ring = control.ring.then(|| {
            if reduced {
                return RING_WIDTH;
            }
            let grow = motions.value(&key(Part::RingGrow)).unwrap_or(1.0) as f32;
            match motions.value(&key(Part::RingShrink)) {
                Some(shrink) => RING_ACTIVE + (RING_WIDTH - RING_ACTIVE) * shrink as f32,
                None => RING_ACTIVE * grow,
            }
        });

        let ripples = control
            .ripples
            .iter()
            .filter(|ripple| ripple.started)
            .map(|ripple| {
                let value = |part| motions.value(&key(part)).unwrap_or(0.0) as f32;
                let slot = ripple.slot;
                let alpha = if ripple.fading {
                    value(Part::RippleOut(slot))
                } else {
                    value(Part::RippleIn(slot))
                };
                RippleLook {
                    at: ripple.at,
                    size: ripple.size,
                    alpha,
                    grow: value(Part::RippleGrow(slot)),
                    center: value(Part::RippleCenter(slot)),
                }
            })
            .collect();

        ControlLook {
            hovered: hover as f32,
            focused: focus as f32,
            pressed: press as f32,
            checked: checked as f32,
            checked_shape: checked_shape as f32,
            elevation: elevation.max(0.0) as f32,
            outline: outline.max(0.0) as f32,
            corners,
            over: control.hovered,
            ring,
            ripples,
        }
    }
}

/// Frame interval while something moves, and while idle-but-alive.
pub const FAST: Duration = Duration::from_millis(16);
pub const SLOW: Duration = Duration::from_millis(50);

// ------------------------------------------------------------------ colour

/// `a` to `b` by `t` (clamped to 0..1), per sRGB channel.
pub fn mix(a: u32, b: u32, t: f64) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xff) as f64, ((b >> shift) & 0xff) as f64);
        ((x + (y - x) * t).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// `row` faded toward `bg` (t = 0 invisible, 1 as painted).
pub fn fade(row: &mut [Run], bg: u32, t: f64) {
    if t >= 1.0 {
        return;
    }
    for run in row.iter_mut() {
        run.paint.fg = mix(bg, run.paint.fg, t);
        if let Some(own) = run.paint.bg {
            run.paint.bg = Some(mix(bg, own, t));
        }
    }
}

/// `row`'s backgrounds from `from` to what they are, by `t` (a highlight
/// arriving); runs without a background are left alone.
pub fn tint(row: &mut [Run], from: u32, t: f64) {
    if t >= 1.0 {
        return;
    }
    for run in row.iter_mut() {
        if let Some(own) = run.paint.bg {
            run.paint.bg = Some(mix(from, own, t));
        }
    }
}

/// How many of `len` rows an opening at `t` shows (at least one while open).
pub fn reveal(len: usize, t: f64) -> usize {
    if t <= 0.0 {
        return 0;
    }
    ((len as f64 * t.min(1.0)).ceil() as usize)
        .clamp(1, len.max(1))
        .min(len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Paint, run};

    const SPRINGS: [Spring; 6] = [
        Spring::GLIDE,
        Spring::FADE,
        Spring::POP,
        Spring::COUNT,
        Spring {
            response: 0.2,
            damping: 1.6,
        },
        Spring {
            response: 0.3,
            damping: 0.3,
        },
    ];

    /// Reference: RK4 on x'' = -w²y - 2zw·v with a tiny step.
    fn numeric(spring: Spring, mut y: f64, mut v: f64, dt: f64) -> (f64, f64) {
        let w = spring.omega();
        let z = spring.damping;
        let accel = |y: f64, v: f64| -w * w * y - 2.0 * z * w * v;
        let steps = 20_000;
        let h = dt / steps as f64;
        for _ in 0..steps {
            let (k1y, k1v) = (v, accel(y, v));
            let (k2y, k2v) = (
                v + h / 2.0 * k1v,
                accel(y + h / 2.0 * k1y, v + h / 2.0 * k1v),
            );
            let (k3y, k3v) = (
                v + h / 2.0 * k2v,
                accel(y + h / 2.0 * k2y, v + h / 2.0 * k2v),
            );
            let (k4y, k4v) = (v + h * k3v, accel(y + h * k3y, v + h * k3v));
            y += h / 6.0 * (k1y + 2.0 * k2y + 2.0 * k3y + k4y);
            v += h / 6.0 * (k1v + 2.0 * k2v + 2.0 * k3v + k4v);
        }
        (y, v)
    }

    #[test]
    fn closed_form_matches_numeric_integration_for_every_damping() {
        for spring in SPRINGS {
            for (y, v) in [(1.0, 0.0), (-3.0, 4.0), (0.0, 10.0), (12.0, -40.0)] {
                for dt in [0.004, 0.05, 0.2] {
                    let exact = spring.solve(y, v, dt);
                    let reference = numeric(spring, y, v, dt);
                    assert!(
                        (exact.0 - reference.0).abs() < 1e-7
                            && (exact.1 - reference.1).abs() < 1e-5,
                        "{spring:?} y={y} v={v} dt={dt}: {exact:?} vs {reference:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn same_motion_at_any_frame_rate() {
        for spring in SPRINGS {
            let mut coarse = Motion {
                value: 10.0,
                velocity: -5.0,
                target: 0.0,
                spring,
                tween: None,
            };
            let mut fine = coarse;
            coarse.step(0.03);
            for _ in 0..30 {
                fine.step(0.001);
            }
            assert!((coarse.value - fine.value).abs() < 1e-9, "{spring:?}");
            assert!((coarse.velocity - fine.velocity).abs() < 1e-7, "{spring:?}");
        }
    }

    /// Time until a unit step settles, and the largest overshoot on the way.
    fn run_step(spring: Spring) -> (f64, f64, f64) {
        let mut motion = Motion {
            value: 0.0,
            velocity: 0.0,
            target: 1.0,
            spring,
            tween: None,
        };
        let (mut t, mut peak, mut ninety) = (0.0, 0.0f64, None);
        while !motion.resting() {
            motion.step(1.0 / 240.0);
            t += 1.0 / 240.0;
            peak = peak.max(motion.value);
            if ninety.is_none() && motion.value >= 0.9 {
                ninety = Some(t);
            }
            assert!(t < 2.0, "{spring:?} never settles");
        }
        (t, peak - 1.0, ninety.unwrap_or(t))
    }

    #[test]
    fn presets_are_fast_and_clear() {
        for spring in [Spring::GLIDE, Spring::FADE, Spring::POP] {
            let (settle, overshoot, ninety) = run_step(spring);
            assert!(ninety < 0.12, "{spring:?}: 90 % after {ninety:.3}s");
            assert!(settle < 0.32, "{spring:?}: at rest after {settle:.3}s");
            if spring.damping >= 1.0 {
                assert!(overshoot <= 1e-9, "{spring:?} overshoots {overshoot}");
            } else {
                assert!(
                    (0.02..0.09).contains(&overshoot),
                    "{spring:?} overshoot {overshoot}"
                );
            }
        }
        let (settle, overshoot, _) = run_step(Spring::COUNT);
        assert!(settle < 0.6 && overshoot <= 1e-9);
    }

    #[test]
    fn expressive_springs_are_the_spec() {
        // Compose's expressive scheme, (stiffness, damping) at mass 1.
        for (spring, stiffness, damping) in [
            (Spring::FAST_SPATIAL, 800.0, 0.6),
            (Spring::FAST_EFFECTS, 3800.0, 1.0),
            (Spring::DEFAULT_EFFECTS, 1600.0, 1.0),
        ] {
            let want = Spring::from_stiffness(stiffness, damping);
            assert!(
                (spring.response - want.response).abs() < 1e-12,
                "{spring:?}"
            );
            assert_eq!(spring.damping, want.damping);
        }
        // The effects springs meet the house bounds and never overshoot.
        for spring in [Spring::FAST_EFFECTS, Spring::DEFAULT_EFFECTS] {
            let (settle, overshoot, ninety) = run_step(spring);
            assert!(ninety < 0.12, "{spring:?}: 90 % after {ninety:.3}s");
            assert!(settle < 0.3, "{spring:?}: at rest after {settle:.3}s");
            assert!(overshoot <= 1e-9);
        }
        // Fast spatial is the spec's bouncy one: quick, about 9.5 % over
        // (beyond POP's 2-9 %, on purpose: it is Material's, not ours).
        let (settle, overshoot, ninety) = run_step(Spring::FAST_SPATIAL);
        assert!(ninety < 0.12, "90 % after {ninety:.3}s");
        assert!(settle < 0.4, "at rest after {settle:.3}s");
        assert!((0.09..0.10).contains(&overshoot), "overshoot {overshoot}");
    }

    #[test]
    fn state_eases_are_the_spec() {
        assert_eq!(Ease::STATE_LAYER, Ease::new(15, LINEAR));
        assert_eq!(Ease::STATE_LAYER_IN, Ease::new(45, LINEAR));
        assert_eq!(Ease::ELEVATION_IN.ms, 120);
        assert_eq!(Ease::ELEVATION_OUT.ms, 150);
        assert_eq!(Ease::RIPPLE_IN.ms + Ease::RIPPLE_GROW.ms, 300);
        assert_eq!(Ease::FOCUS_RING_GROW.ms + Ease::FOCUS_RING_SHRINK.ms, 600);
        assert!((LINEAR.eval(0.3) - 0.3).abs() < 1e-9);
        for ease in [Ease::STATE_LAYER, Ease::ELEVATION_IN, Ease::RIPPLE_GROW] {
            assert!(ease.curve.eval(1.0) > 0.999 && ease.ms <= 225);
        }
    }

    #[test]
    fn critically_damped_never_reverses() {
        let mut motion = Motion {
            value: 0.0,
            velocity: 0.0,
            target: 40.0,
            spring: Spring::GLIDE,
            tween: None,
        };
        let mut last = motion.value;
        while !motion.resting() {
            motion.step(1.0 / 60.0);
            assert!(
                motion.value >= last - 1e-12,
                "{} after {}",
                motion.value,
                last
            );
            last = motion.value;
        }
        assert_eq!(motion.value, 40.0, "lands exactly");
    }

    #[test]
    fn retarget_keeps_velocity() {
        let mut motions = Motions::new(false);
        let t0 = Instant::now();
        motions.tick(t0);
        motions.follow(Key::Scroll, 0.0, Spring::GLIDE);
        motions.follow(Key::Scroll, 20.0, Spring::GLIDE);
        motions.tick(t0 + Duration::from_millis(16));
        motions.tick(t0 + Duration::from_millis(48));
        let before = motions.motions[&Key::Scroll];
        assert!(before.velocity > 0.0 && before.value > 0.0 && before.value < 20.0);
        motions.follow(Key::Scroll, -10.0, Spring::GLIDE);
        let after = motions.motions[&Key::Scroll];
        assert_eq!(
            (after.value, after.velocity),
            (before.value, before.velocity),
            "continuous"
        );
        // Right after the turn it still moves on (velocity is continuous)...
        motions.tick(t0 + Duration::from_millis(50));
        let next = motions.motions[&Key::Scroll];
        assert!(
            next.value > before.value,
            "momentum carries it on before turning"
        );
        // ...then heads for the new target.
        motions.tick(t0 + Duration::from_millis(66));
        let turned = motions.motions[&Key::Scroll];
        assert!(
            turned.value < next.value && turned.velocity < 0.0,
            "{turned:?}"
        );
    }

    #[test]
    fn follow_animates_changes_and_rests() {
        let mut motions = Motions::new(false);
        let t0 = Instant::now();
        motions.tick(t0);
        assert_eq!(
            motions.follow(Key::Context, 0.4, Spring::COUNT),
            0.4,
            "first value as is"
        );
        assert!(!motions.active());
        assert_eq!(
            motions.follow(Key::Context, 0.8, Spring::COUNT),
            0.4,
            "change starts here"
        );
        assert!(motions.active());
        let mut t = t0;
        let mut seen = Vec::new();
        for _ in 0..120 {
            t += Duration::from_millis(16);
            motions.tick(t);
            seen.push(motions.follow(Key::Context, 0.8, Spring::COUNT));
        }
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "monotonic");
        assert!(seen[2] > 0.4 && seen[2] < 0.8, "in between at frame 3");
        assert_eq!(*seen.last().unwrap(), 0.8);
        assert!(!motions.active());
    }

    #[test]
    fn a_frame_after_idle_does_not_skip_the_start() {
        let mut motions = Motions::new(false);
        let t0 = Instant::now();
        motions.tick(t0);
        motions.follow(Key::Notice, 1.0, Spring::FADE);
        // No frames for ten seconds, then one in which something appears.
        let shown = t0 + Duration::from_secs(10);
        motions.tick(shown);
        motions.start(Key::Notice, 0.0, 1.0, Spring::FADE);
        assert_eq!(
            motions.value(&Key::Notice),
            Some(0.0),
            "first frame shows the start"
        );
        motions.tick(shown + Duration::from_millis(16));
        let first = motions.value(&Key::Notice).unwrap();
        assert!(first > 0.0 && first < 0.5);
        // A long stall mid-motion advances at most one capped step.
        motions.tick(t0 + Duration::from_secs(20));
        let stalled = motions.value(&Key::Notice).unwrap();
        assert!(stalled < 1.0, "{stalled}: capped, not finished");
    }

    #[test]
    fn nudge_and_snap() {
        let mut motions = Motions::new(false);
        let t0 = Instant::now();
        motions.tick(t0);
        motions.follow(Key::Scroll, 0.0, Spring::GLIDE);
        motions.nudge(Key::Scroll, 3.0);
        assert_eq!(
            motions.value(&Key::Scroll),
            Some(3.0),
            "content stays where it was"
        );
        motions.tick(t0 + Duration::from_millis(16));
        let mid = motions.value(&Key::Scroll).unwrap();
        assert!(mid < 3.0 && mid > 0.0, "{mid}");
        motions.snap(Key::Scroll, 0.0);
        assert!(!motions.active());
        motions.nudge(Key::Context, 1.0);
        assert_eq!(
            motions.value(&Key::Context),
            None,
            "nudging nothing is a no-op"
        );
    }

    #[test]
    fn finished_rows_are_dropped() {
        let mut motions = Motions::new(false);
        let mut t = Instant::now();
        motions.tick(t);
        for row in 0..50 {
            motions.start(Key::Row(row), 0.0, 1.0, Spring::FADE);
        }
        for _ in 0..60 {
            t += Duration::from_millis(16);
            motions.tick(t);
        }
        assert!(motions.motions.is_empty(), "{} left", motions.motions.len());
        motions.start(Key::Row(1), 0.0, 1.0, Spring::FADE);
        motions.forget_rows();
        assert!(motions.motions.is_empty());
    }

    #[test]
    fn reduced_motion_snaps() {
        let mut motions = Motions::new(true);
        motions.tick(Instant::now());
        motions.follow(Key::Scroll, 0.0, Spring::GLIDE);
        assert_eq!(motions.follow(Key::Scroll, 30.0, Spring::GLIDE), 30.0);
        motions.start(Key::Row(4), 0.0, 1.0, Spring::FADE);
        assert_eq!(motions.value(&Key::Row(4)), Some(1.0));
        motions.nudge(Key::Scroll, 5.0);
        assert_eq!(motions.value(&Key::Scroll), Some(30.0));
        assert!(!motions.active());
    }

    #[test]
    fn colours_mix_per_channel() {
        assert_eq!(mix(0x000000, 0xffffff, 0.0), 0x000000);
        assert_eq!(mix(0x000000, 0xffffff, 1.0), 0xffffff);
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(mix(0x102030, 0x102030, 0.37), 0x102030);
        assert_eq!(mix(0xff0000, 0x0000ff, 0.25), 0xbf0040);
        assert_eq!(mix(0x000000, 0xffffff, 1.4), 0xffffff, "overshoot clamps");
        assert_eq!(mix(0x000000, 0xffffff, -2.0), 0x000000);
    }

    #[test]
    fn rows_fade_tint_and_reveal() {
        let row = || {
            vec![
                run("a", Paint::fg(0xffffff)),
                run("b", Paint::fg(0x808080).on(0x404040)),
            ]
        };
        let mut gone = row();
        fade(&mut gone, 0x000000, 0.0);
        assert!(
            gone.iter()
                .all(|r| r.paint.fg == 0 && r.paint.bg.is_none_or(|bg| bg == 0))
        );
        let mut full = row();
        fade(&mut full, 0x000000, 1.0);
        assert_eq!(full, row());
        let mut half = row();
        fade(&mut half, 0x000000, 0.5);
        assert_eq!(
            (half[0].paint.fg, half[1].paint.bg),
            (0x808080, Some(0x202020))
        );
        assert_eq!(half[0].text, "a", "text untouched");

        let mut lit = row();
        tint(&mut lit, 0x000000, 0.5);
        assert_eq!(
            (lit[0].paint, lit[1].paint.bg),
            (row()[0].paint, Some(0x202020))
        );

        assert_eq!(reveal(6, 0.0), 0);
        assert_eq!(reveal(6, 0.01), 1);
        assert_eq!(reveal(6, 0.5), 3);
        assert_eq!(reveal(6, 1.08), 6, "overshoot shows all, never more");
        assert_eq!(reveal(0, 1.0), 0);
    }
}
