//! What every Material 3 component shares: focus, hover, press and the
//! keyboard wired into `shroom_ui::motion::Controls` (which animates them on
//! the spec's curves), and the paint of the result: the state layer, the
//! press ripple clipped to the container's rounded shape, the outline, the
//! focus ring (material-web) and the elevation shadow (material-web's
//! formula). Components build on [`Ui`] and [`overlay`]; none animates or
//! paints a state of its own.

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    AnyElement, App, BorderStyle, Bounds, BoxShadow, ClickEvent, Div, FocusHandle, FontWeight,
    Hsla, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, PathBuilder, Pixels, SharedString,
    Stateful, Window, canvas, div, point, prelude::*, px, quad, rgb,
};
use shroom_ui::chat::Chat;
use shroom_ui::motion::{ControlEvent, ControlLook, ControlSpec, RippleLook};

use crate::m3::{self, Scheme, font::Type};

/// Reports a component's input to the window (which hands it to `Chat`).
pub type Sink = Rc<dyn Fn(u64, ControlEvent, &mut Window, &mut App)>;
/// Each component's container bounds as last painted, by control id.
pub type Places = Rc<RefCell<HashMap<u64, Bounds<Pixels>>>>;
/// What a click does.
pub type OnClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A stable control id for a component key (FNV-1a).
pub fn control_id(key: &str) -> u64 {
    key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// One frame's view of the window's components.
pub struct Ui<'a> {
    chat: RefCell<&'a mut Chat>,
    handles: RefCell<&'a mut HashMap<u64, FocusHandle>>,
    used: RefCell<HashSet<u64>>,
    focused: Option<FocusHandle>,
    keyboard: bool,
    places: Places,
    sink: Sink,
    /// Components shown focused whatever holds focus (the gallery).
    pinned: HashSet<u64>,
    /// Where focus goes when a component is clicked or let go of (the
    /// composer: typing always lands there).
    pub home: FocusHandle,
    pub zoom: f32,
    /// Device pixels per logical pixel (layout rounds to these).
    pub device: f32,
    pub scheme: &'static Scheme,
}

impl<'a> Ui<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chat: &'a mut Chat,
        handles: &'a mut HashMap<u64, FocusHandle>,
        window: &Window,
        cx: &App,
        places: Places,
        sink: Sink,
        home: FocusHandle,
        zoom: f32,
        scheme: &'static Scheme,
    ) -> Self {
        Self {
            chat: RefCell::new(chat),
            handles: RefCell::new(handles),
            used: RefCell::default(),
            focused: window.focused(cx),
            keyboard: window.last_input_was_keyboard(),
            places,
            sink,
            home,
            zoom,
            device: window.scale_factor(),
            scheme,
            pinned: HashSet::new(),
        }
    }

    /// Show components `ids` focused, as keyboard focus would (the
    /// gallery's focused samples; only one element can hold real focus).
    pub fn with_pinned_focus(mut self, ids: HashSet<u64>) -> Self {
        self.pinned = ids;
        self
    }

    pub fn chat(&self) -> Ref<'_, &'a mut Chat> {
        self.chat.borrow()
    }

    /// Run `f` on the chat (for components that read or change it).
    pub fn with_chat<T>(&self, f: impl FnOnce(&mut Chat) -> T) -> T {
        f(&mut self.chat.borrow_mut())
    }

    /// Component `id`'s animated look this frame.
    pub fn look(&self, id: u64, spec: ControlSpec) -> ControlLook {
        self.chat.borrow_mut().control_look(id, spec)
    }

    /// Text field `id`'s animated look this frame.
    pub fn field(
        &self,
        id: u64,
        spec: shroom_ui::motion::FieldSpec,
    ) -> shroom_ui::motion::FieldLook {
        self.chat.borrow_mut().field_look(id, spec)
    }

    /// Component `id`'s focus handle (a tab stop), kept while it is drawn.
    pub fn handle(&self, id: u64, cx: &mut App) -> FocusHandle {
        self.used.borrow_mut().insert(id);
        self.handles
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| cx.focus_handle().tab_index(0).tab_stop(true))
            .clone()
    }

    /// Keyboard focus is on `handle` (focus-visible: not a click's focus).
    pub fn focus_visible(&self, handle: &FocusHandle) -> bool {
        self.keyboard && self.focused.as_ref() == Some(handle)
    }

    /// Keyboard focus is on component `id` (or the gallery pins it there).
    pub fn focus_visible_id(&self, id: u64, cx: &mut App) -> bool {
        let handle = self.handle(id, cx);
        self.focus_visible(&handle) || self.pinned.contains(&id)
    }

    pub fn places(&self) -> &Places {
        &self.places
    }

    /// Drop the focus handles of components not drawn this frame.
    /// Component `id` is no longer drawn: it starts afresh next time.
    pub fn forget(&self, id: u64) {
        self.chat.borrow_mut().forget_control(id);
    }

    pub fn finish(self) {
        let used = self.used.into_inner();
        self.handles.into_inner().retain(|id, _| used.contains(id));
    }
}

/// Report the pointer entering and leaving `target` as component `id`'s
/// hover (a container that is not itself a control: a menu group).
pub fn hoverable(target: Stateful<Div>, ui: &Ui, id: u64) -> Stateful<Div> {
    let sink = ui.sink.clone();
    target.on_hover(move |on, window, cx| sink(id, ControlEvent::Hover(*on), window, cx))
}

/// What a component is to accessibility.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Semantics {
    Button,
    /// A plain item of a menu or list, with this role.
    Item(gpui::Role),
    /// A toggle (checkbox role), on or off.
    Toggle(bool),
    /// One of a set where one is selected (`Role::Tab`, `Role::RadioButton`).
    Selectable {
        role: gpui::Role,
        selected: bool,
    },
}

/// Wire a component's interactive root: tab stop, hover, press (pointer and
/// Enter/Space), release, click and its accessibility role.
#[allow(clippy::too_many_arguments)]
pub fn interact(
    target: Stateful<Div>,
    ui: &Ui,
    id: u64,
    handle: Option<&FocusHandle>,
    aria: gpui::SharedString,
    semantics: Semantics,
    on_click: Option<OnClick>,
) -> Stateful<Div> {
    let mut target = target.aria_label(aria);
    target = match semantics {
        Semantics::Button => target.role(gpui::Role::Button),
        Semantics::Item(role) => target.role(role),
        Semantics::Toggle(on) => target.role(gpui::Role::CheckBox).aria_toggled(if on {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        }),
        Semantics::Selectable { role, selected } => target.role(role).aria_selected(selected),
    };
    let Some(handle) = handle else {
        // Disabled: no focus, no states, and the click stops here.
        return target.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    };
    let event = |sink: &Sink, places: &Places| {
        let (sink, places) = (sink.clone(), places.clone());
        move |at: Option<(f32, f32)>, release: bool, window: &mut Window, cx: &mut App| {
            if release {
                sink(id, ControlEvent::Release, window, cx);
                return;
            }
            let bounds = places.borrow().get(&id).copied();
            let size = bounds.map_or((0., 0.), |b| {
                (f32::from(b.size.width), f32::from(b.size.height))
            });
            let at = match (at, bounds) {
                (Some((x, y)), Some(b)) => (x - f32::from(b.origin.x), y - f32::from(b.origin.y)),
                _ => (size.0 / 2., size.1 / 2.),
            };
            sink(id, ControlEvent::Press { at, size }, window, cx);
        }
    };
    let hover = ui.sink.clone();
    let down = event(&ui.sink, &ui.places);
    let up = event(&ui.sink, &ui.places);
    let up_out = event(&ui.sink, &ui.places);
    let key_down = event(&ui.sink, &ui.places);
    let key_up = event(&ui.sink, &ui.places);
    let home = ui.home.clone();
    let activation = |key: &str, modified: bool| (key == "enter" || key == "space") && !modified;
    target = target
        .track_focus(handle)
        .cursor_pointer()
        .on_hover(move |on, window, cx| hover(id, ControlEvent::Hover(*on), window, cx))
        .on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window, cx| {
                // The click lands here, and keyboard focus stays with typing.
                window.prevent_default();
                cx.stop_propagation();
                let at = (f32::from(event.position.x), f32::from(event.position.y));
                down(Some(at), false, window, cx);
                window.focus(&home, cx);
            },
        )
        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
            up(None, true, window, cx)
        })
        .on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
            up_out(None, true, window, cx)
        })
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            let stroke = &event.keystroke;
            if activation(&stroke.key, stroke.modifiers.modified()) && !event.is_held {
                key_down(None, false, window, cx);
            }
        })
        .on_key_up(move |event: &KeyUpEvent, window, cx| {
            let stroke = &event.keystroke;
            if activation(&stroke.key, stroke.modifiers.modified()) {
                key_up(None, true, window, cx);
            }
        });
    if let Some(on_click) = on_click {
        target = target.on_click(move |event, window, cx| on_click(event, window, cx));
    }
    target
}

// ---------------------------------------------------------------- controls

/// The minimum interactive size (Compose `minimumInteractiveComponentSize`,
/// material-web's touch target): every control's target is at least this.
pub const TARGET: f32 = 48.;

/// What every control's builder takes alike: its accessible label, click,
/// keyboard shortcut and whether it is enabled. A builder holds one as
/// `act` and gets the setters from [`act_setters!`].
#[derive(Clone)]
pub struct Act {
    pub aria: Option<SharedString>,
    pub on_click: Option<OnClick>,
    pub keys: Option<SharedString>,
    pub enabled: bool,
}

impl Default for Act {
    fn default() -> Self {
        Self {
            aria: None,
            on_click: None,
            keys: None,
            enabled: true,
        }
    }
}

/// The setters for a builder's `act: Act` field: `aria`, `on_click`,
/// `keyshortcuts` and `enabled`.
macro_rules! act_setters {
    () => {
        /// What a screen reader says (default: the visible label).
        #[allow(dead_code)]
        pub fn aria(mut self, text: impl Into<gpui::SharedString>) -> Self {
            self.act.aria = Some(text.into());
            self
        }

        #[allow(dead_code)]
        pub fn on_click(mut self, f: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static) -> Self {
            self.act.on_click = Some($crate::m3_state::click(f));
            self
        }

        /// The keyboard shortcut a screen reader announces.
        #[allow(dead_code)]
        pub fn keyshortcuts(mut self, keys: impl Into<gpui::SharedString>) -> Self {
            self.act.keys = Some(keys.into());
            self
        }

        /// Disabled: no focus, no states, no click.
        #[allow(dead_code)]
        pub fn enabled(mut self, on: bool) -> Self {
            self.act.enabled = on;
            self
        }
    };
}
pub(crate) use act_setters;

/// One control this frame: its id, tab stop and focus, and the pieces of
/// its element: the animated look, the state layer and the wired root.
/// A component is its tokens, its colours and the content between
/// [`Control::root`] (or [`Control::target`]) and [`Control::wire`].
pub struct Control {
    pub id: u64,
    key: SharedString,
    handle: Option<FocusHandle>,
    pub focused: bool,
    act: Act,
}

impl Control {
    pub fn new(key: SharedString, act: Act, ui: &Ui, cx: &mut App) -> Self {
        let id = control_id(&key);
        let handle = act.enabled.then(|| ui.handle(id, cx));
        let focused = handle
            .as_ref()
            .is_some_and(|handle| ui.focus_visible(handle) || ui.pinned.contains(&id));
        Self {
            id,
            key,
            handle,
            focused,
            act,
        }
    }

    pub fn enabled(&self) -> bool {
        self.act.enabled
    }

    /// The animated look for `spec` (its `enabled` and `focused` are
    /// this control's).
    pub fn look(&self, ui: &Ui, spec: ControlSpec) -> ControlLook {
        ui.look(
            self.id,
            ControlSpec {
                enabled: self.act.enabled,
                focused: self.focused,
                ..spec
            },
        )
    }

    /// Its state layer: `look`'s states over corners `radius` (px) in
    /// `content`, the colour of what it holds.
    pub fn overlay(
        &self,
        ui: &Ui,
        look: ControlLook,
        radius: impl Into<Radii>,
        content: u32,
    ) -> Overlay {
        Overlay::new(self.id, look, radius, content, ui)
    }

    /// The root element (it holds the container).
    pub fn root(&self) -> Stateful<Div> {
        div().id(gpui::ElementId::Name(self.key.clone()))
    }

    /// A root at least [`TARGET`] each way, its container centred in it
    /// (for containers smaller than the target: buttons, chips).
    pub fn target(&self, zoom: f32) -> Stateful<Div> {
        self.root()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .min_w(px(TARGET * zoom))
            .min_h(px(TARGET * zoom))
    }

    /// Wire `root` (from [`Control::root`] or [`Control::target`]): tab
    /// stop, states, click, role and label (`label` unless `aria` set one).
    pub fn wire(
        self,
        root: Stateful<Div>,
        ui: &Ui,
        label: SharedString,
        semantics: Semantics,
    ) -> Stateful<Div> {
        let mut root = root;
        if let Some(keys) = self.act.keys {
            root = root.aria_keyshortcuts(keys);
        }
        interact(
            root,
            ui,
            self.id,
            self.handle.as_ref(),
            self.act.aria.unwrap_or(label),
            semantics,
            self.act.on_click,
        )
    }
}

/// A component's colours that move between two states (unselected and
/// selected) together: each `Tone` mixed by the same amount.
pub trait Mix: Copy {
    fn mix(self, to: Self, t: f32) -> Self;
}

/// `impl Mix` for a colours struct: `tones` mix, `keep` fields stay
/// `self`'s (plain colour roles that switch rather than glide).
macro_rules! mix_tones {
    ($name:ident { $($tone:ident),* $(,)? } $(keep { $($keep:ident),* $(,)? })?) => {
        impl $crate::m3_state::Mix for $name {
            fn mix(self, to: Self, t: f32) -> Self {
                Self {
                    $($tone: self.$tone.mix(to.$tone, t),)*
                    $($($keep: self.$keep,)*)?
                }
            }
        }
    };
}
pub(crate) use mix_tones;

/// Type-scale styling for any element.
pub trait TypeStyle: Styled + Sized {
    /// Set in `ty`: the sans, its weight, size and line height, at `zoom`.
    fn type_style(self, ty: Type, zoom: f32) -> Self {
        self.font_family(m3::SANS)
            .font_weight(ty.weight)
            .text_size(px(ty.size * zoom))
            .line_height(px(ty.line * zoom))
    }

    /// One line, ending in an ellipsis where it runs out of room.
    fn one_line(self) -> Self {
        self.overflow_hidden().text_ellipsis().whitespace_nowrap()
    }
}

impl<E: Styled> TypeStyle for E {}

// ------------------------------------------------------------------ pieces

/// A Material Symbol at `size` (outlined, or filled), in `tone`.
pub fn icon_glyph(
    glyph: &'static str,
    size: f32,
    filled: bool,
    tone: Tone,
    zoom: f32,
) -> AnyElement {
    icon_fill(glyph, size, if filled { 1. } else { 0. }, tone, zoom)
}

/// A Material Symbol `fill` of the way from outlined (0) to filled (1).
/// Material Symbols animate their Fill axis on a state change (selection);
/// gpui draws static faces, so the filled face fades in over the outlined
/// one, which it covers, on the caller's spring.
pub fn icon_fill(glyph: &'static str, size: f32, fill: f32, tone: Tone, zoom: f32) -> AnyElement {
    let fill = fill.clamp(0., 1.);
    let face = |family: &'static str| {
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .font_family(family)
            .font_weight(FontWeight::NORMAL)
            .text_size(px(size * zoom))
            .line_height(px(size * zoom))
            .text_color(tone.hsla())
            .child(glyph)
    };
    div()
        .flex_none()
        .relative()
        .size(px(size * zoom))
        .when(fill < 1., |icon| icon.child(face(m3::SYMBOLS)))
        .when(fill > 0., |icon| {
            icon.child(face(m3::SYMBOLS_FILLED).opacity(fill))
        })
        .into_any_element()
}

/// One line of text in a type-scale style.
pub fn label_text(text: SharedString, ty: Type, tone: Tone, zoom: f32) -> AnyElement {
    div()
        .flex_none()
        .whitespace_nowrap()
        .type_style(ty, zoom)
        .text_color(tone.hsla())
        .child(text)
        .into_any_element()
}

/// Lengths along one axis of something drawn at a changing scale (a menu
/// scaling open). Laid out piece by piece, each height rounds to device
/// pixels on its own, and when the scale carries several over a half pixel
/// together the whole jumps by that many pixels in one frame. Each piece
/// here is the difference of its two rounded ends instead, so the whole
/// moves by at most a pixel at a time and every piece stays within one.
pub struct Ruler {
    zoom: f32,
    device: f32,
    at: f32,
}

impl Ruler {
    /// `zoom` logical pixels per dp, `device` device pixels per logical.
    pub fn new(zoom: f32, device: f32) -> Self {
        Self {
            zoom,
            device: device.max(1e-3),
            at: 0.,
        }
    }

    /// The next `dp` along, in logical pixels (whole device pixels).
    pub fn take(&mut self, dp: f32) -> gpui::Pixels {
        let edge = |at: f32| (at * self.zoom * self.device).round();
        let from = edge(self.at);
        self.at += dp;
        px((edge(self.at) - from) / self.device)
    }
}

/// A line's advance width in `ty` (dp, unzoomed): for sizing a container
/// before layout. Kerning is left out, so allow a pixel or two.
pub fn text_width(text: &str, ty: Type, cx: &App) -> f32 {
    let system = cx.text_system();
    let mut font = gpui::font(m3::SANS);
    font.weight = ty.weight;
    let id = system.resolve_font(&font);
    text.chars()
        .map(|ch| {
            system
                .advance(id, px(ty.size), ch)
                .map_or(ty.size * 0.6, |size| f32::from(size.width))
        })
        .sum()
}

/// A click handler from a closure that needs only the window and app.
pub fn click(f: impl Fn(&mut Window, &mut App) + 'static) -> OnClick {
    std::rc::Rc::new(move |_: &ClickEvent, window: &mut Window, cx: &mut App| f(window, cx))
}

// ------------------------------------------------------------------ colour

/// A colour role at an opacity (`a` 0: absent, e.g. no container).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    pub rgb: u32,
    pub a: f32,
}

impl Tone {
    pub const NONE: Tone = Tone { rgb: 0, a: 0. };

    pub const fn of(rgb: u32) -> Self {
        Self { rgb, a: 1. }
    }

    pub const fn at(rgb: u32, a: f32) -> Self {
        Self { rgb, a }
    }

    pub fn hsla(self) -> Hsla {
        let mut color: Hsla = rgb(self.rgb).into();
        color.a = self.a;
        color
    }

    /// `self` toward `to` by `t` (an absent colour takes the other's hue).
    pub fn mix(self, to: Tone, t: f32) -> Tone {
        let t = t.clamp(0., 1.);
        let rgb = if self.a <= 0. {
            to.rgb
        } else if to.a <= 0. {
            self.rgb
        } else {
            shroom_ui::motion::mix(self.rgb, to.rgb, f64::from(t))
        };
        Tone {
            rgb,
            a: self.a + (to.a - self.a) * t,
        }
    }
}

impl Mix for Tone {
    fn mix(self, to: Tone, t: f32) -> Tone {
        Tone::mix(self, to, t)
    }
}

// ------------------------------------------------------------------ paint

/// Elevation `level` (fractional while it moves) as gpui shadows, by
/// material-web's formula (`_elevation.scss`): a key light at 30 % and an
/// ambient one at 15 %. CSS blur is twice gpui's (a Gaussian sigma).
pub fn shadows(level: f32, zoom: f32, shadow: u32) -> Vec<BoxShadow> {
    if level <= 0.001 {
        return Vec::new();
    }
    let l = level;
    let c = |v: f32, hi: f32| v.clamp(0., hi);
    let key_y = c(l, 1.) + c(l - 3., 1.) + 2. * c(l - 4., 1.);
    let key_blur = 2. * c(l, 1.) + c(l - 2., 1.) + c(l - 4., 1.);
    let ambient_y = c(l, 1.) + c(l - 1., 1.) + 2. * c(l - 2., 3.);
    let ambient_blur = 3. * c(l, 2.) + 2. * c(l - 2., 3.);
    let ambient_spread = c(l, 4.) + 2. * c(l - 4., 1.);
    let shade = |a: f32| Tone::at(shadow, a).hsla();
    vec![
        BoxShadow {
            color: shade(0.3),
            offset: point(px(0.), px(key_y * zoom)),
            blur_radius: px(key_blur * zoom / 2.),
            spread_radius: px(0.),
            inset: false,
        },
        BoxShadow {
            color: shade(0.15),
            offset: point(px(0.), px(ambient_y * zoom)),
            blur_radius: px(ambient_blur * zoom / 2.),
            spread_radius: px(ambient_spread * zoom),
            inset: false,
        },
    ]
}

/// What [`overlay`] paints over a container.
#[derive(Clone, Debug)]
pub struct Overlay {
    pub id: u64,
    /// Corner radii now, px (zoomed).
    pub radius: Radii,
    /// The content colour: state layer and ripple.
    pub content: u32,
    pub look: ControlLook,
    /// Outline width (px, zoomed) and colour.
    pub outline: Option<(f32, Tone)>,
    pub zoom: f32,
    pub ring: u32,
}

impl Overlay {
    /// `look`'s states over corners `radius` (px) in `content`, at the
    /// window's zoom, the ring in the scheme's `FocusIndicatorColor`
    /// (Secondary), no outline.
    pub fn new(
        id: u64,
        look: ControlLook,
        radius: impl Into<Radii>,
        content: u32,
        ui: &Ui,
    ) -> Self {
        Self {
            id,
            radius: radius.into(),
            content,
            look,
            outline: None,
            zoom: ui.zoom,
            ring: ui.scheme.secondary,
        }
    }

    /// An outline `width` dp wide (none below a hundredth), at this zoom.
    pub fn outline(mut self, width: f32, tone: Tone) -> Self {
        self.outline = (width > 0.01).then_some((width * self.zoom, tone));
        self
    }

    /// Drawn at `zoom` instead of the window's (a menu scaling open).
    pub fn zoom(mut self, zoom: f32) -> Self {
        self.zoom = zoom;
        self
    }
}

/// Corner radii, px: the top two corners and the bottom two.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Radii {
    pub top: f32,
    pub bottom: f32,
}

impl From<f32> for Radii {
    fn from(r: f32) -> Self {
        Self { top: r, bottom: r }
    }
}

impl Radii {
    /// A look's animated corners at `zoom`, each clamped to half the
    /// height (Compose's `CornerSize` clamps the same way).
    pub fn of(corners: shroom_ui::motion::Corners, zoom: f32, height: f32) -> Self {
        Self {
            top: (corners.top * zoom).min(height * zoom / 2.),
            bottom: (corners.bottom * zoom).min(height * zoom / 2.),
        }
    }

    /// For gpui's quads.
    pub fn corners(self) -> gpui::Corners<Pixels> {
        gpui::Corners {
            top_left: px(self.top),
            top_right: px(self.top),
            bottom_right: px(self.bottom),
            bottom_left: px(self.bottom),
        }
    }

    /// Each radius grown by `by` (a ring around it).
    pub fn grow(self, by: f32) -> Self {
        Self {
            top: self.top + by,
            bottom: self.bottom + by,
        }
    }
}

/// Record an element's bounds as `id`'s place, painting nothing (an
/// absolute child filling it).
pub fn measure(places: &Places, id: u64) -> impl IntoElement {
    let places = places.clone();
    canvas(
        move |bounds, _, _| {
            places.borrow_mut().insert(id, bounds);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Focus ring geometry (material-web): outside the container by this much.
pub const RING_OFFSET: f32 = 2.;
/// Ripple radius beyond the half diagonal (Compose `RippleAnimation`).
pub const RIPPLE_REACH: f32 = 10.;

/// The layer over a component's container (absolute, filling it): records
/// its bounds for presses and paints the states `over` describes.
pub fn overlay(places: &Places, over: Overlay) -> impl IntoElement {
    let places = places.clone();
    let id = over.id;
    canvas(
        move |bounds, _, _| {
            places.borrow_mut().insert(id, bounds);
            bounds
        },
        move |_, bounds, window, _| paint(bounds, &over, window),
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

fn paint(bounds: Bounds<Pixels>, over: &Overlay, window: &mut Window) {
    let look = &over.look;
    let r = over.radius;
    let layer = (m3::HOVER * look.hovered).max(m3::FOCUS * look.focused);
    if layer > 0.001 {
        window.paint_quad(quad(
            bounds,
            r.corners(),
            Tone::at(over.content, layer).hsla(),
            px(0.),
            gpui::transparent_black(),
            BorderStyle::Solid,
        ));
    }
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    for ripple in &look.ripples {
        let alpha = m3::PRESS * ripple.alpha;
        if alpha <= 0.001 {
            continue;
        }
        let (cx, cy, radius) = ripple_circle(ripple, (w, h), over.zoom);
        let shape = clip(
            &circle((x + cx, y + cy), radius),
            &rounded_rect((x, y, w, h), r),
        );
        fill(window, &shape, Tone::at(over.content, alpha).hsla());
    }
    if let Some((width, tone)) = over.outline
        && width > 0.01
    {
        window.paint_quad(quad(
            bounds,
            r.corners(),
            gpui::transparent_black(),
            px(width),
            tone.hsla(),
            BorderStyle::Solid,
        ));
    }
    if let Some(width) = look.ring
        && width > 0.01
    {
        let out = (RING_OFFSET + width) * over.zoom;
        window.paint_quad(quad(
            bounds.dilate(px(out)),
            r.grow(out).corners(),
            gpui::transparent_black(),
            px(width * over.zoom),
            Tone::of(over.ring).hsla(),
            BorderStyle::Solid,
        ));
    }
}

/// A ripple's centre (from the container's corner) and radius now: from the
/// press point at 0.3 of the longer side, to the centre, covering it all.
pub fn ripple_circle(ripple: &RippleLook, size: (f32, f32), zoom: f32) -> (f32, f32, f32) {
    let (w, h) = size;
    let start = w.max(h) * 0.3;
    let end = (w * w + h * h).sqrt() / 2. + RIPPLE_REACH * zoom;
    let to = (w / 2., h / 2.);
    let t = ripple.center;
    (
        ripple.at.0 + (to.0 - ripple.at.0) * t,
        ripple.at.1 + (to.1 - ripple.at.1) * t,
        start + (end - start) * ripple.grow,
    )
}

type Polygon = Vec<(f32, f32)>;

fn circle(centre: (f32, f32), radius: f32) -> Polygon {
    let n = 64;
    (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            (centre.0 + radius * a.cos(), centre.1 + radius * a.sin())
        })
        .collect()
}

/// A rounded rectangle as a convex polygon (each corner in 8 steps).
pub fn rounded_rect((x, y, w, h): (f32, f32, f32, f32), radius: impl Into<Radii>) -> Polygon {
    let radius = radius.into();
    let top = radius.top.clamp(0., w.min(h) / 2.);
    let bottom = radius.bottom.clamp(0., w.min(h) / 2.);
    let corners = [
        (x + w - top, y + top, -90.0f32, top),
        (x + w - bottom, y + h - bottom, 0., bottom),
        (x + bottom, y + h - bottom, 90., bottom),
        (x + top, y + top, 180., top),
    ];
    let mut points = Vec::new();
    for (cx, cy, from, r) in corners {
        for step in 0..=8 {
            let a = (from + 90. * step as f32 / 8.).to_radians();
            points.push((cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    points
}

/// `subject` cut to the convex `clip` (Sutherland–Hodgman).
pub fn clip(subject: &[(f32, f32)], clip: &[(f32, f32)]) -> Polygon {
    let area: f32 = clip
        .iter()
        .zip(clip.iter().cycle().skip(1))
        .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
        .sum();
    let sign = area.signum();
    let inside = |a: (f32, f32), b: (f32, f32), p: (f32, f32)| {
        ((b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)) * sign >= 0.
    };
    let cross = |a: (f32, f32), b: (f32, f32), p: (f32, f32), q: (f32, f32)| {
        let (dx, dy) = (q.0 - p.0, q.1 - p.1);
        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
        let denom = ex * dy - ey * dx;
        if denom.abs() < f32::EPSILON {
            return p;
        }
        let t = (ex * (a.1 - p.1) - ey * (a.0 - p.0)) / denom;
        (p.0 + dx * t, p.1 + dy * t)
    };
    let mut out: Polygon = subject.to_vec();
    for i in 0..clip.len() {
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let input = std::mem::take(&mut out);
        for j in 0..input.len() {
            let (p, q) = (input[(j + input.len() - 1) % input.len()], input[j]);
            match (inside(a, b, p), inside(a, b, q)) {
                (true, true) => out.push(q),
                (true, false) => out.push(cross(a, b, p, q)),
                (false, true) => {
                    out.push(cross(a, b, p, q));
                    out.push(q);
                }
                (false, false) => {}
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

fn fill(window: &mut Window, shape: &[(f32, f32)], color: Hsla) {
    let [first, rest @ ..] = shape else {
        return;
    };
    let mut path = PathBuilder::fill();
    path.move_to(point(px(first.0), px(first.1)));
    for p in rest {
        path.line_to(point(px(p.0), px(p.1)));
    }
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Pair {
        a: Tone,
        b: Tone,
        tag: u8,
    }
    mix_tones!(Pair { a, b } keep { tag });

    #[test]
    fn mix_tones_mixes_every_tone_and_keeps_the_rest() {
        let from = Pair {
            a: Tone::of(0x000000),
            b: Tone::of(0xffffff),
            tag: 1,
        };
        let to = Pair {
            a: Tone::of(0xffffff),
            b: Tone::of(0x000000),
            tag: 2,
        };
        assert_eq!(from.mix(to, 0.), from);
        assert_eq!(from.mix(to, 1.), Pair { tag: 1, ..to });
        let half = from.mix(to, 0.5);
        assert_eq!(half.a, Tone::of(0x000000).mix(Tone::of(0xffffff), 0.5));
        assert_eq!(half.b, Tone::of(0xffffff).mix(Tone::of(0x000000), 0.5));
        assert_eq!(half.tag, 1);
    }

    #[test]
    fn an_overlay_outline_is_zoomed_and_dropped_when_hairline() {
        let bare = || Overlay {
            id: 1,
            radius: Radii::from(4.),
            content: 0xffffff,
            look: ControlLook::default(),
            outline: None,
            zoom: 2.,
            ring: 0,
        };
        let tone = Tone::of(0x808080);
        let drawn = bare().outline(1., tone);
        assert_eq!(drawn.outline, Some((2., tone)));
        let gone = bare().outline(0.005, tone);
        assert_eq!(gone.outline, None);
        // The width is taken at the zoom in force when it is set.
        let menu = bare().zoom(0.5).outline(1., tone);
        assert_eq!(menu.outline, Some((0.5, tone)));
    }

    #[test]
    fn controls_take_the_48_dp_target() {
        assert_eq!(TARGET, 48.);
    }

    #[test]
    fn shadows_follow_material_web() {
        assert!(shadows(0., 1., 0).is_empty());
        // Level 1: key 1px down, 2px CSS blur; ambient 1px down, 3px blur, 1px spread.
        let one = shadows(1., 1., 0);
        assert_eq!(one[0].offset.y, px(1.));
        assert_eq!(one[0].blur_radius, px(1.));
        assert_eq!(one[1].offset.y, px(1.));
        assert_eq!(one[1].blur_radius, px(1.5));
        assert_eq!(one[1].spread_radius, px(1.));
        assert!((one[0].color.a - 0.3).abs() < 1e-6 && (one[1].color.a - 0.15).abs() < 1e-6);
        // Level 3 (FAB): key 1px/3px, ambient 4px/8px spread 3px (CSS).
        let three = shadows(3., 1., 0);
        assert_eq!(three[0].offset.y, px(1.));
        assert_eq!(three[0].blur_radius, px(1.5));
        assert_eq!(three[1].offset.y, px(4.));
        assert_eq!(three[1].blur_radius, px(4.));
        assert_eq!(three[1].spread_radius, px(3.));
        // Zoom scales everything.
        assert_eq!(shadows(1., 2., 0)[1].spread_radius, px(2.));
    }

    #[test]
    fn ripples_start_at_the_press_and_cover_the_container() {
        let at = |center, grow| RippleLook {
            at: (10., 5.),
            size: (100., 40.),
            alpha: 1.,
            grow,
            center,
        };
        let (x, y, r) = ripple_circle(&at(0., 0.), (100., 40.), 1.);
        assert_eq!((x, y), (10., 5.));
        assert!((r - 30.).abs() < 1e-4);
        let (x, y, r) = ripple_circle(&at(1., 1.), (100., 40.), 1.);
        assert_eq!((x, y), (50., 20.));
        let half_diagonal = (100f32 * 100. + 40. * 40.).sqrt() / 2.;
        assert!((r - half_diagonal - 10.).abs() < 1e-4);
    }

    #[test]
    fn ripples_are_clipped_to_rounded_corners() {
        let shape = rounded_rect((0., 0., 100., 40.), 20.);
        // A huge circle clipped to the pill is the pill.
        let cut = clip(&circle((50., 20.), 500.), &shape);
        for p in &cut {
            assert!(p.0 >= -0.01 && p.0 <= 100.01 && p.1 >= -0.01 && p.1 <= 40.01);
            // Nothing in the cut-away corner.
            assert!(!(p.0 < 4. && p.1 < 4.), "{p:?}");
        }
        // Fully inside: unchanged.
        let small = circle((50., 20.), 5.);
        assert_eq!(clip(&small, &shape).len(), small.len());
        // Fully outside: nothing.
        assert!(clip(&circle((500., 500.), 5.), &shape).is_empty());
    }

    #[test]
    fn tones_mix_through_absent_colours() {
        let none = Tone::NONE;
        let ink = Tone::of(0x336699);
        assert_eq!(none.mix(ink, 0.5), Tone::at(0x336699, 0.5));
        assert_eq!(ink.mix(none, 1.), Tone::at(0x336699, 0.));
        assert_eq!(ink.mix(Tone::of(0), 0.), ink);
    }

    #[test]
    fn control_ids_are_stable() {
        assert_eq!(control_id("send"), control_id("send"));
        assert_ne!(control_id("send"), control_id("attach"));
    }
}
