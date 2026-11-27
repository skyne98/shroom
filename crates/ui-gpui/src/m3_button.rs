//! Material 3 Expressive buttons, to the token values of AndroidX Compose
//! Material 3 (`Button*Tokens`, `IconButton*Tokens`, `Fab*Tokens`,
//! `ExtendedFab*Tokens`, `*ButtonTokens` colours) and its `ButtonDefaults`,
//! `ToggleButtonDefaults`, `IconButtonDefaults` and
//! `FloatingActionButtonDefaults`: common buttons (filled, tonal, elevated,
//! outlined, text) in five sizes, round or square, with toggle variants;
//! icon buttons (standard, filled, tonal, outlined) in five sizes and three
//! widths, with toggle variants; FABs and extended FABs.
//!
//! States (hover, focus, press, ripple, shape morph, elevation, focus ring)
//! are animated by `shroom_ui::motion` and painted by `m3_state`.

use gpui::{AnyElement, App, SharedString, div, prelude::*, px};
use shroom_ui::motion::{ControlLook, ControlSpec, Spring};

use crate::m3::{self, Scheme, font::Type, level, shape, space};
use crate::m3_state::{
    Act, Control, Mix, Semantics, Tone, Ui, act_setters, icon_fill, icon_glyph, label_text,
    mix_tones, overlay, shadows,
};
/// `ButtonDefaults.MinWidth`.
pub const MIN_WIDTH: f32 = 58.;

// ------------------------------------------------------------------- sizes

/// The five button sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    XSmall,
    Small,
    Medium,
    Large,
    XLarge,
}

/// A container's resting corners: fully round, or the size's square shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Round,
    Square,
}

/// One size of common button (`Button<Size>Tokens` with `ButtonDefaults`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonMetrics {
    pub height: f32,
    pub leading: f32,
    pub trailing: f32,
    pub icon: f32,
    pub gap: f32,
    pub label: Type,
    /// Corners: square at rest, pressed, checked when round and when square.
    pub square: f32,
    pub pressed: f32,
    pub checked_round: f32,
    pub checked_square: f32,
    pub outline: f32,
}

impl Size {
    pub fn button(self) -> ButtonMetrics {
        use m3::font::*;
        let (height, pad, icon, gap, label, square, pressed, checked_square, outline) = match self {
            Size::XSmall => (
                32.,
                12.,
                20.,
                space::S50,
                LABEL_LARGE,
                shape::MEDIUM,
                shape::SMALL,
                shape::FULL,
                1.,
            ),
            Size::Small => (
                40.,
                space::S200,
                20.,
                space::S100,
                LABEL_LARGE,
                shape::MEDIUM,
                shape::SMALL,
                shape::MEDIUM,
                1.,
            ),
            Size::Medium => (
                56.,
                space::S300,
                24.,
                space::S100,
                TITLE_MEDIUM,
                shape::LARGE,
                shape::MEDIUM,
                shape::FULL,
                1.,
            ),
            Size::Large => (
                96.,
                space::S600,
                32.,
                space::S150,
                HEADLINE_SMALL,
                shape::EXTRA_LARGE,
                shape::LARGE,
                shape::FULL,
                2.,
            ),
            Size::XLarge => (
                136.,
                space::S800,
                40.,
                space::S200,
                HEADLINE_LARGE,
                shape::EXTRA_LARGE,
                shape::LARGE,
                shape::FULL,
                3.,
            ),
        };
        ButtonMetrics {
            height,
            leading: pad,
            trailing: pad,
            icon,
            gap,
            label,
            square,
            pressed,
            // `SelectedContainerShapeRound` equals each size's square shape.
            checked_round: square,
            checked_square,
            outline,
        }
    }

    pub fn icon_button(self) -> IconButtonMetrics {
        // (height, icon, default/narrow/wide space, square, pressed, outline)
        let (height, icon, spaces, square, pressed, outline) = match self {
            Size::XSmall => (
                32.,
                20.,
                [space::S75, space::S50, space::S125],
                shape::MEDIUM,
                shape::SMALL,
                1.,
            ),
            Size::Small => (
                40.,
                24.,
                [space::S100, space::S50, space::S175],
                shape::MEDIUM,
                shape::SMALL,
                1.,
            ),
            Size::Medium => (
                56.,
                24.,
                [space::S200, space::S150, space::S300],
                shape::LARGE,
                shape::MEDIUM,
                1.,
            ),
            Size::Large => (
                96.,
                32.,
                [space::S400, space::S200, space::S600],
                shape::EXTRA_LARGE,
                shape::LARGE,
                2.,
            ),
            Size::XLarge => (
                136.,
                40.,
                [space::S600, space::S400, space::S900],
                shape::EXTRA_LARGE,
                shape::LARGE,
                3.,
            ),
        };
        IconButtonMetrics {
            height,
            icon,
            spaces,
            square,
            pressed,
            checked_round: square,
            checked_square: shape::FULL,
            outline,
        }
    }
}

/// One size of icon button (`IconButton<Size>Tokens`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconButtonMetrics {
    pub height: f32,
    pub icon: f32,
    /// Space on each side of the icon: uniform, narrow, wide.
    pub spaces: [f32; 3],
    pub square: f32,
    pub pressed: f32,
    pub checked_round: f32,
    pub checked_square: f32,
    pub outline: f32,
}

/// An icon button's width (`IconButtonWidthOption`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    Uniform,
    Narrow,
    Wide,
}

impl IconButtonMetrics {
    pub fn width(&self, width: Width) -> f32 {
        let space = match width {
            Width::Uniform => self.spaces[0],
            Width::Narrow => self.spaces[1],
            Width::Wide => self.spaces[2],
        };
        self.icon + 2. * space
    }
}

/// Corners now: rest toward checked by `checked`, then toward pressed.
pub fn corners(rest: f32, checked: f32, pressed: f32, look: &ControlLook, height: f32) -> f32 {
    let r = |corner: f32| shape::radius(corner, height);
    let base = r(rest) + (r(checked) - r(rest)) * look.checked_shape;
    base + (r(pressed) - base) * look.pressed
}

// ------------------------------------------------------------------ colour

/// The common button styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Filled,
    Tonal,
    Elevated,
    Outlined,
    Text,
}

/// The icon button styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    Standard,
    Filled,
    Tonal,
    Outlined,
}

/// A component's colours in one state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub container: Tone,
    pub label: Tone,
    pub icon: Tone,
    pub outline: Tone,
}

impl Colors {
    fn plain(container: Tone, content: u32, outline: Tone) -> Self {
        Self {
            container,
            label: Tone::of(content),
            icon: Tone::of(content),
            outline,
        }
    }
}

/// `kind`'s colours: `None` plain, `Some(checked)` as a toggle.
pub fn button_colors(kind: Kind, toggle: Option<bool>, s: &Scheme) -> Colors {
    use Kind::*;
    let none = Tone::NONE;
    let outline = Tone::of(s.outline_variant);
    match (kind, toggle) {
        (Filled, None | Some(true)) => Colors::plain(Tone::of(s.primary), s.on_primary, none),
        (Filled, Some(false)) => {
            Colors::plain(Tone::of(s.surface_container), s.on_surface_variant, none)
        }
        (Tonal, None | Some(false)) => Colors::plain(
            Tone::of(s.secondary_container),
            s.on_secondary_container,
            none,
        ),
        (Tonal, Some(true)) => Colors::plain(Tone::of(s.secondary), s.on_secondary, none),
        (Elevated, None | Some(false)) => {
            Colors::plain(Tone::of(s.surface_container_low), s.primary, none)
        }
        (Elevated, Some(true)) => Colors::plain(Tone::of(s.primary), s.on_primary, none),
        (Outlined, None | Some(false)) => Colors::plain(none, s.on_surface_variant, outline),
        (Outlined, Some(true)) => {
            Colors::plain(Tone::of(s.inverse_surface), s.inverse_on_surface, none)
        }
        (Text, _) => Colors::plain(none, s.primary, none),
    }
}

/// `kind`'s disabled colours (`Disabled*` tokens).
pub fn button_disabled(kind: Kind, checked: bool, s: &Scheme) -> Colors {
    let container = match kind {
        Kind::Outlined if checked => Tone::at(s.on_surface, m3::DISABLED_CONTAINER),
        Kind::Outlined | Kind::Text => Tone::NONE,
        _ => Tone::at(s.on_surface, m3::DISABLED_CONTAINER),
    };
    let outline = match kind {
        Kind::Outlined if !checked => Tone::at(s.outline_variant, m3::DISABLED_CONTAINER),
        _ => Tone::NONE,
    };
    Colors {
        container,
        label: Tone::at(s.on_surface_variant, m3::DISABLED_CONTENT),
        icon: Tone::at(s.on_surface, m3::DISABLED_CONTENT),
        outline,
    }
}

/// `kind`'s icon button colours: `None` plain, `Some(checked)` as a toggle.
pub fn icon_colors(kind: IconKind, toggle: Option<bool>, s: &Scheme) -> Colors {
    use IconKind::*;
    let none = Tone::NONE;
    match (kind, toggle) {
        (Standard, None | Some(false)) => Colors::plain(none, s.on_surface_variant, none),
        (Standard, Some(true)) => Colors::plain(none, s.primary, none),
        (Filled, None | Some(true)) => Colors::plain(Tone::of(s.primary), s.on_primary, none),
        (Filled, Some(false)) => {
            Colors::plain(Tone::of(s.surface_container), s.on_surface_variant, none)
        }
        (Tonal, None | Some(false)) => Colors::plain(
            Tone::of(s.secondary_container),
            s.on_secondary_container,
            none,
        ),
        (Tonal, Some(true)) => Colors::plain(Tone::of(s.secondary), s.on_secondary, none),
        (Outlined, None | Some(false)) => {
            Colors::plain(none, s.on_surface_variant, Tone::of(s.outline_variant))
        }
        (Outlined, Some(true)) => {
            Colors::plain(Tone::of(s.inverse_surface), s.inverse_on_surface, none)
        }
    }
}

/// `kind`'s disabled icon button colours.
pub fn icon_disabled(kind: IconKind, checked: bool, s: &Scheme) -> Colors {
    let container = match kind {
        IconKind::Filled | IconKind::Tonal => Tone::at(s.on_surface, m3::DISABLED_CONTAINER),
        IconKind::Outlined if checked => Tone::at(s.on_surface, m3::DISABLED_CONTAINER),
        _ => Tone::NONE,
    };
    let outline = match kind {
        IconKind::Outlined if !checked => Tone::at(s.outline_variant, m3::DISABLED_CONTENT),
        _ => Tone::NONE,
    };
    let content = Tone::at(s.on_surface, m3::DISABLED_CONTENT);
    Colors {
        container,
        label: content,
        icon: content,
        outline,
    }
}

/// Elevation levels at rest, hovered, focused and pressed.
pub fn button_elevation(kind: Kind) -> [f32; 4] {
    match kind {
        Kind::Filled | Kind::Tonal => [level::L0, level::L1, level::L0, level::L0],
        Kind::Elevated => [level::L1, level::L2, level::L1, level::L1],
        Kind::Outlined | Kind::Text => [level::L0; 4],
    }
}

mix_tones!(Colors {
    container,
    label,
    icon,
    outline
});

/// A toggle's colours now: unchecked toward checked by `look.checked`.
fn toggled(off: Colors, on: Colors, look: &ControlLook) -> Colors {
    off.mix(on, look.checked)
}

// ------------------------------------------------------------------ pieces

/// Everything a component's container needs to draw itself.
struct Frame {
    control: Control,
    /// The visible label, or what the icon means.
    label: SharedString,
    toggle: Option<bool>,
    height: f32,
    /// Fixed width, or padding on each side.
    width: Option<f32>,
    padding: (f32, f32),
    min_width: f32,
    gap: f32,
    radius: f32,
    colors: Colors,
    outline: f32,
    elevation: f32,
    look: ControlLook,
    children: Vec<AnyElement>,
}

fn frame(ui: &Ui, f: Frame) -> AnyElement {
    let z = ui.zoom;
    let mut container = div()
        .relative()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(f.gap * z))
        .h(px(f.height * z))
        .rounded(px(f.radius))
        .bg(f.colors.container.hsla())
        .shadow(shadows(f.elevation, z, ui.scheme.shadow));
    container = match f.width {
        Some(width) => container.w(px(width * z)),
        None => container
            .min_w(px(f.min_width * z))
            .pl(px(f.padding.0 * z))
            .pr(px(f.padding.1 * z)),
    };
    let over = f
        .control
        .overlay(ui, f.look, f.radius, f.colors.label.rgb)
        .outline(f.outline, f.colors.outline);
    let container = container
        .child(overlay(ui.places(), over))
        .children(f.children);
    let target = f.control.target(z);
    let semantics = f.toggle.map_or(Semantics::Button, Semantics::Toggle);
    f.control
        .wire(target, ui, f.label, semantics)
        .child(container)
        .into_any_element()
}

// ------------------------------------------------------------------ Button

/// A common button, or with `toggle` a toggle button.
pub struct Button {
    key: SharedString,
    kind: Kind,
    size: Size,
    shape: Shape,
    label: SharedString,
    icon: Option<&'static str>,
    toggle: Option<bool>,
    act: Act,
}

impl Button {
    act_setters!();

    pub fn new(key: impl Into<SharedString>, kind: Kind, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            kind,
            size: Size::Small,
            shape: Shape::Round,
            label: label.into(),
            icon: None,
            toggle: None,
            act: Act::default(),
        }
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub fn shape(mut self, shape: Shape) -> Self {
        self.shape = shape;
        self
    }

    /// A leading icon (a Material Symbols glyph).
    pub fn icon(mut self, glyph: &'static str) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// A toggle button, checked or not (Text buttons have no toggle).
    pub fn toggle(mut self, checked: bool) -> Self {
        debug_assert!(self.kind != Kind::Text, "text buttons do not toggle");
        self.toggle = Some(checked);
        self
    }

    /// Padding (leading, trailing): `contentPaddingFor`, and the text
    /// button's own (`TextButtonContentPadding`) at the default size.
    pub fn padding(&self) -> (f32, f32) {
        let m = self.size.button();
        if self.kind == Kind::Text && self.size == Size::Small {
            let end = if self.icon.is_some() {
                space::S200
            } else {
                space::S150
            };
            return (space::S150, end);
        }
        (m.leading, m.trailing)
    }

    pub fn render(self, ui: &Ui, cx: &mut App) -> AnyElement {
        let s = ui.scheme;
        let m = self.size.button();
        let padding = self.padding();
        let control = Control::new(self.key, self.act, ui, cx);
        let checked = self.toggle == Some(true);
        let spec = ControlSpec {
            checked,
            elevation: button_elevation(self.kind),
            outline: if self.kind == Kind::Outlined && !checked {
                m.outline
            } else {
                0.
            },
            // Buttons morph on the effects spring; toggles on fast spatial.
            shape: if self.toggle.is_some() {
                Spring::FAST_SPATIAL
            } else {
                Spring::DEFAULT_EFFECTS
            },
            ..ControlSpec::default()
        };
        let look = control.look(ui, spec);
        let colors = if !control.enabled() {
            button_disabled(self.kind, checked, s)
        } else if self.toggle.is_some() {
            toggled(
                button_colors(self.kind, Some(false), s),
                button_colors(self.kind, Some(true), s),
                &look,
            )
        } else {
            button_colors(self.kind, None, s)
        };
        let (rest, checked_corner) = match self.shape {
            Shape::Round => (shape::FULL, m.checked_round),
            Shape::Square => (m.square, m.checked_square),
        };
        let radius = corners(rest, checked_corner, m.pressed, &look, m.height) * ui.zoom;
        let mut children = Vec::new();
        if let Some(glyph) = self.icon {
            children.push(icon_glyph(glyph, m.icon, false, colors.icon, ui.zoom));
        }
        children.push(label_text(
            self.label.clone(),
            m.label,
            colors.label,
            ui.zoom,
        ));
        frame(
            ui,
            Frame {
                control,
                label: self.label,
                toggle: self.toggle,
                height: m.height,
                width: None,
                padding,
                min_width: MIN_WIDTH,
                gap: m.gap,
                radius,
                colors,
                outline: look.outline,
                elevation: look.elevation,
                look,
                children,
            },
        )
    }
}

// -------------------------------------------------------------- IconButton

/// An icon button, or with `toggle` an icon toggle button.
pub struct IconButton {
    key: SharedString,
    kind: IconKind,
    size: Size,
    shape: Shape,
    width: Width,
    icon: &'static str,
    /// Draw the icon filled while checked (Material's selected icons).
    fill_checked: bool,
    toggle: Option<bool>,
    aria: SharedString,
    /// A standard icon button's content colour where it sits (Compose's
    /// `LocalContentColor`, e.g. an app bar's leading icon colour).
    content: Option<u32>,
    act: Act,
}

impl IconButton {
    act_setters!();

    /// `aria` names it for assistive technology (it has no visible label).
    pub fn new(
        key: impl Into<SharedString>,
        kind: IconKind,
        icon: &'static str,
        aria: impl Into<SharedString>,
    ) -> Self {
        Self {
            key: key.into(),
            kind,
            size: Size::Small,
            shape: Shape::Round,
            width: Width::Uniform,
            icon,
            fill_checked: false,
            toggle: None,
            aria: aria.into(),
            content: None,
            act: Act::default(),
        }
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub fn shape(mut self, shape: Shape) -> Self {
        self.shape = shape;
        self
    }

    /// The local content colour a standard icon button takes (unchecked).
    pub fn content(mut self, rgb: u32) -> Self {
        debug_assert!(self.kind == IconKind::Standard);
        self.content = Some(rgb);
        self
    }

    pub fn width(mut self, width: Width) -> Self {
        self.width = width;
        self
    }

    pub fn toggle(mut self, checked: bool) -> Self {
        self.toggle = Some(checked);
        self
    }

    pub fn fill_checked(mut self) -> Self {
        self.fill_checked = true;
        self
    }

    pub fn render(self, ui: &Ui, cx: &mut App) -> AnyElement {
        let s = ui.scheme;
        let m = self.size.icon_button();
        let control = Control::new(self.key, self.act, ui, cx);
        let enabled = control.enabled();
        let checked = self.toggle == Some(true);
        let spec = ControlSpec {
            checked,
            elevation: [0.; 4],
            outline: if self.kind == IconKind::Outlined && !checked {
                m.outline
            } else {
                0.
            },
            shape: Spring::DEFAULT_EFFECTS,
            ..ControlSpec::default()
        };
        let look = control.look(ui, spec);
        let colors = if !enabled {
            icon_disabled(self.kind, checked, s)
        } else if self.toggle.is_some() {
            toggled(
                icon_colors(self.kind, Some(false), s),
                icon_colors(self.kind, Some(true), s),
                &look,
            )
        } else {
            icon_colors(self.kind, None, s)
        };
        let colors = match self.content {
            Some(rgb) if enabled && self.toggle.is_none() => Colors {
                label: Tone::of(rgb),
                icon: Tone::of(rgb),
                ..colors
            },
            Some(rgb) if enabled => Colors {
                label: Tone::of(rgb).mix(colors.label, look.checked),
                icon: Tone::of(rgb).mix(colors.icon, look.checked),
                ..colors
            },
            _ => colors,
        };
        let (rest, checked_corner) = match self.shape {
            Shape::Round => (shape::FULL, m.checked_round),
            Shape::Square => (m.square, m.checked_square),
        };
        let radius = corners(rest, checked_corner, m.pressed, &look, m.height) * ui.zoom;
        let fill = if self.fill_checked { look.checked } else { 0. };
        let children = vec![icon_fill(self.icon, m.icon, fill, colors.icon, ui.zoom)];
        frame(
            ui,
            Frame {
                control,
                label: self.aria,
                toggle: self.toggle,
                height: m.height,
                width: Some(m.width(self.width)),
                padding: (0., 0.),
                min_width: 0.,
                gap: 0.,
                radius,
                colors,
                outline: look.outline,
                elevation: 0.,
                look,
                children,
            },
        )
    }
}

// --------------------------------------------------------------------- FAB

/// FAB sizes (`Fab<Size>Tokens`, `ExtendedFab<Size>Tokens`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FabSize {
    /// FAB only (40); extended FABs start at Small = 56.
    Mini,
    Small,
    Medium,
    Large,
}

/// A FAB's colour set (`Fab<Color>Tokens`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FabColor {
    PrimaryContainer,
    SecondaryContainer,
    TertiaryContainer,
    Primary,
    Secondary,
    Tertiary,
}

/// One FAB size: (height, corner, icon) and, extended, (label, gap, padding).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FabMetrics {
    pub height: f32,
    pub corner: f32,
    pub icon: f32,
    pub label: Type,
    pub gap: f32,
    pub padding: f32,
}

impl FabSize {
    pub fn fab(self) -> FabMetrics {
        use m3::font::*;
        let (height, corner, icon) = match self {
            FabSize::Mini => (40., shape::MEDIUM, 24.),
            FabSize::Small => (56., shape::LARGE, 24.),
            FabSize::Medium => (80., shape::LARGE_INCREASED, 28.),
            FabSize::Large => (96., shape::EXTRA_LARGE, 36.),
        };
        let (label, gap, padding) = match self {
            FabSize::Mini | FabSize::Small => (LABEL_LARGE, space::S100, space::S200),
            FabSize::Medium => (TITLE_LARGE, space::S150, 26.),
            FabSize::Large => (HEADLINE_SMALL, space::S200, 28.),
        };
        FabMetrics {
            height,
            corner,
            icon,
            label,
            gap,
            padding,
        }
    }
}

pub fn fab_colors(color: FabColor, s: &Scheme) -> Colors {
    let (container, content) = match color {
        FabColor::PrimaryContainer => (s.primary_container, s.on_primary_container),
        FabColor::SecondaryContainer => (s.secondary_container, s.on_secondary_container),
        FabColor::TertiaryContainer => (s.tertiary_container, s.on_tertiary_container),
        FabColor::Primary => (s.primary, s.on_primary),
        FabColor::Secondary => (s.secondary, s.on_secondary),
        FabColor::Tertiary => (s.tertiary, s.on_tertiary),
    };
    Colors::plain(Tone::of(container), content, Tone::NONE)
}

/// FAB elevation (rest, hover, focus, press): Level3/4/3/3, or lowered
/// (`FloatingActionButtonDefaults.loweredElevation`, on a surface) 1/2/1/1.
pub fn fab_elevation(lowered: bool) -> [f32; 4] {
    if lowered {
        [level::L1, level::L2, level::L1, level::L1]
    } else {
        [level::L3, level::L4, level::L3, level::L3]
    }
}

/// A floating action button; with a label, an extended FAB.
pub struct Fab {
    key: SharedString,
    size: FabSize,
    color: FabColor,
    icon: &'static str,
    label: Option<SharedString>,
    aria: SharedString,
    lowered: bool,
    act: Act,
}

impl Fab {
    act_setters!();

    pub fn new(
        key: impl Into<SharedString>,
        icon: &'static str,
        aria: impl Into<SharedString>,
    ) -> Self {
        Self {
            key: key.into(),
            size: FabSize::Small,
            color: FabColor::PrimaryContainer,
            icon,
            label: None,
            aria: aria.into(),
            lowered: false,
            act: Act::default(),
        }
    }

    /// An extended FAB with this label beside the icon.
    pub fn extended(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn size(mut self, size: FabSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: FabColor) -> Self {
        self.color = color;
        self
    }

    pub fn lowered(mut self) -> Self {
        self.lowered = true;
        self
    }

    pub fn render(self, ui: &Ui, cx: &mut App) -> AnyElement {
        let size = match (self.size, &self.label) {
            (FabSize::Mini, Some(_)) => FabSize::Small,
            (size, _) => size,
        };
        let m = size.fab();
        let control = Control::new(self.key, self.act, ui, cx);
        let spec = ControlSpec {
            elevation: fab_elevation(self.lowered),
            ..ControlSpec::default()
        };
        let look = control.look(ui, spec);
        let colors = fab_colors(self.color, ui.scheme);
        let mut children = vec![icon_glyph(self.icon, m.icon, false, colors.icon, ui.zoom)];
        let (width, padding) = match &self.label {
            Some(label) => {
                children.push(label_text(label.clone(), m.label, colors.label, ui.zoom));
                (None, (m.padding, m.padding))
            }
            None => (Some(m.height), (0., 0.)),
        };
        frame(
            ui,
            Frame {
                control,
                label: self.aria,
                toggle: None,
                height: m.height,
                width,
                padding,
                min_width: 0.,
                gap: m.gap,
                radius: shape::radius(m.corner, m.height) * ui.zoom,
                colors,
                outline: 0.,
                elevation: look.elevation,
                look,
                children,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::m3::M3;

    #[test]
    fn button_sizes_are_the_tokens() {
        let rows = [
            (Size::XSmall, 32., 12., 20., 4., 14., 1.),
            (Size::Small, 40., 16., 20., 8., 14., 1.),
            (Size::Medium, 56., 24., 24., 8., 16., 1.),
            (Size::Large, 96., 48., 32., 12., 24., 2.),
            (Size::XLarge, 136., 64., 40., 16., 32., 3.),
        ];
        for (size, height, pad, icon, gap, text, outline) in rows {
            let m = size.button();
            assert_eq!(
                (
                    m.height,
                    m.leading,
                    m.trailing,
                    m.icon,
                    m.gap,
                    m.label.size,
                    m.outline
                ),
                (height, pad, pad, icon, gap, text, outline),
                "{size:?}"
            );
        }
        assert_eq!(Size::Medium.button().label, m3::font::TITLE_MEDIUM);
        assert_eq!(Size::XLarge.button().label.line, 40.);
    }

    #[test]
    fn button_shapes_are_the_tokens() {
        let shapes = |s: Size| {
            let m = s.button();
            (m.square, m.pressed, m.checked_round, m.checked_square)
        };
        assert_eq!(shapes(Size::XSmall), (12., 8., 12., shape::FULL));
        assert_eq!(shapes(Size::Small), (12., 8., 12., 12.));
        assert_eq!(shapes(Size::Medium), (16., 12., 16., shape::FULL));
        assert_eq!(shapes(Size::Large), (28., 16., 28., shape::FULL));
        assert_eq!(shapes(Size::XLarge), (28., 16., 28., shape::FULL));
    }

    #[test]
    fn icon_button_sizes_and_widths_are_the_tokens() {
        let rows = [
            (Size::XSmall, 32., 20., [32., 28., 40.]),
            (Size::Small, 40., 24., [40., 32., 52.]),
            (Size::Medium, 56., 24., [56., 48., 72.]),
            (Size::Large, 96., 32., [96., 64., 128.]),
            (Size::XLarge, 136., 40., [136., 104., 184.]),
        ];
        for (size, height, icon, widths) in rows {
            let m = size.icon_button();
            assert_eq!((m.height, m.icon), (height, icon), "{size:?}");
            let got = [Width::Uniform, Width::Narrow, Width::Wide].map(|w| m.width(w));
            assert_eq!(got, widths, "{size:?}");
            assert_eq!(m.checked_square, shape::FULL);
        }
        assert_eq!(Size::Small.icon_button().pressed, shape::SMALL);
        assert_eq!(Size::Medium.icon_button().checked_round, shape::LARGE);
        assert_eq!(Size::Large.icon_button().outline, 2.);
    }

    #[test]
    fn corners_morph_from_rest_through_checked_to_pressed() {
        let mut look = ControlLook::default();
        // Fully round at rest: half the height.
        assert_eq!(corners(shape::FULL, 12., 8., &look, 40.), 20.);
        look.checked_shape = 1.;
        assert_eq!(corners(shape::FULL, 12., 8., &look, 40.), 12.);
        look.pressed = 1.;
        assert_eq!(corners(shape::FULL, 12., 8., &look, 40.), 8.);
        look.pressed = 0.5;
        assert_eq!(corners(shape::FULL, 12., 8., &look, 40.), 10.);
    }

    #[test]
    fn button_colours_are_the_tokens() {
        let s = &M3;
        let c = |k, t| button_colors(k, t, s);
        assert_eq!(c(Kind::Filled, None).container, Tone::of(s.primary));
        assert_eq!(c(Kind::Filled, None).label, Tone::of(s.on_primary));
        assert_eq!(
            c(Kind::Tonal, None).label,
            Tone::of(s.on_secondary_container)
        );
        assert_eq!(
            c(Kind::Elevated, None).container,
            Tone::of(s.surface_container_low)
        );
        assert_eq!(c(Kind::Elevated, None).label, Tone::of(s.primary));
        assert_eq!(c(Kind::Outlined, None).container, Tone::NONE);
        assert_eq!(c(Kind::Outlined, None).outline, Tone::of(s.outline_variant));
        assert_eq!(c(Kind::Text, None).label, Tone::of(s.primary));
        // Toggles.
        assert_eq!(
            c(Kind::Filled, Some(false)).container,
            Tone::of(s.surface_container)
        );
        assert_eq!(
            c(Kind::Filled, Some(false)).label,
            Tone::of(s.on_surface_variant)
        );
        assert_eq!(c(Kind::Tonal, Some(true)).container, Tone::of(s.secondary));
        assert_eq!(c(Kind::Tonal, Some(true)).label, Tone::of(s.on_secondary));
        assert_eq!(c(Kind::Elevated, Some(true)).container, Tone::of(s.primary));
        let outlined_on = c(Kind::Outlined, Some(true));
        assert_eq!(outlined_on.container, Tone::of(s.inverse_surface));
        assert_eq!(outlined_on.outline, Tone::NONE, "no border when checked");
        // Disabled.
        let off = button_disabled(Kind::Filled, false, s);
        assert_eq!(off.container, Tone::at(s.on_surface, 0.1));
        assert_eq!(off.label, Tone::at(s.on_surface_variant, 0.38));
        assert_eq!(off.icon, Tone::at(s.on_surface, 0.38));
        assert_eq!(button_disabled(Kind::Text, false, s).container, Tone::NONE);
        assert_eq!(
            button_disabled(Kind::Outlined, false, s).outline,
            Tone::at(s.outline_variant, 0.1)
        );
        assert_eq!(
            button_disabled(Kind::Outlined, true, s).container,
            Tone::at(s.on_surface, 0.1)
        );
    }

    #[test]
    fn icon_button_colours_are_the_tokens() {
        let s = &M3;
        let c = |k, t| icon_colors(k, t, s);
        assert_eq!(
            c(IconKind::Standard, None).icon,
            Tone::of(s.on_surface_variant)
        );
        assert_eq!(c(IconKind::Standard, Some(true)).icon, Tone::of(s.primary));
        assert_eq!(c(IconKind::Filled, None).container, Tone::of(s.primary));
        assert_eq!(
            c(IconKind::Filled, Some(false)).container,
            Tone::of(s.surface_container)
        );
        assert_eq!(
            c(IconKind::Tonal, Some(true)).icon,
            Tone::of(s.on_secondary)
        );
        assert_eq!(
            c(IconKind::Outlined, None).outline,
            Tone::of(s.outline_variant)
        );
        assert_eq!(
            c(IconKind::Outlined, Some(true)).container,
            Tone::of(s.inverse_surface)
        );
        let off = icon_disabled(IconKind::Filled, false, s);
        assert_eq!(
            (off.container, off.icon),
            (Tone::at(s.on_surface, 0.1), Tone::at(s.on_surface, 0.38))
        );
        assert_eq!(
            icon_disabled(IconKind::Standard, false, s).container,
            Tone::NONE
        );
        assert_eq!(
            icon_disabled(IconKind::Outlined, false, s).outline,
            Tone::at(s.outline_variant, 0.38)
        );
    }

    #[test]
    fn elevations_are_the_tokens() {
        assert_eq!(button_elevation(Kind::Filled), [0., 1., 0., 0.]);
        assert_eq!(button_elevation(Kind::Elevated), [1., 2., 1., 1.]);
        assert_eq!(button_elevation(Kind::Text), [0.; 4]);
        assert_eq!(fab_elevation(false), [3., 4., 3., 3.]);
        assert_eq!(fab_elevation(true), [1., 2., 1., 1.]);
    }

    #[test]
    fn fab_sizes_are_the_tokens() {
        let f = |s: FabSize| {
            let m = s.fab();
            (m.height, m.corner, m.icon)
        };
        assert_eq!(f(FabSize::Mini), (40., 12., 24.));
        assert_eq!(f(FabSize::Small), (56., 16., 24.));
        assert_eq!(f(FabSize::Medium), (80., 20., 28.));
        assert_eq!(f(FabSize::Large), (96., 28., 36.));
        let e = |s: FabSize| {
            let m = s.fab();
            (m.label, m.gap, m.padding)
        };
        assert_eq!(e(FabSize::Small), (m3::font::LABEL_LARGE, 8., 16.));
        assert_eq!(e(FabSize::Medium), (m3::font::TITLE_LARGE, 12., 26.));
        assert_eq!(e(FabSize::Large), (m3::font::HEADLINE_SMALL, 16., 28.));
        let primary = fab_colors(FabColor::PrimaryContainer, &M3);
        assert_eq!(primary.container, Tone::of(M3.primary_container));
        assert_eq!(primary.icon, Tone::of(M3.on_primary_container));
    }

    #[test]
    fn text_buttons_pad_by_their_own_token() {
        let text = Button::new("t", Kind::Text, "Go");
        assert_eq!(text.padding(), (12., 12.));
        assert_eq!(text.icon(m3::icon::ADD).padding(), (12., 16.));
        let filled = Button::new("f", Kind::Filled, "Go").size(Size::XSmall);
        assert_eq!(filled.padding(), (12., 12.));
    }
}
