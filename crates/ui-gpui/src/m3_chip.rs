//! Material 3 Expressive filter chip, to the token values of AndroidX
//! Compose Material 3 (`FilterChipTokens`, `ChipsTokens`) and its
//! `FilterChip(shapes = FilterChipDefaults.shapes())`: 32 tall in a 48
//! target, a leading icon, the label and a trailing icon.
//!
//! The model chip is the sample `ElevatedFilterChipWithCornerMorphingSample`
//! flat: selected while its menu is open, with a trailing ArrowDropDown. Its
//! shape morphs on fast spatial: 12 unselected, fully round selected, 8
//! pressed. As in Compose, the colours switch with selection (not
//! animated); hover, focus and press show through `m3_state`.

use gpui::{AnyElement, App, SharedString, div, prelude::*, px};
use shroom_ui::motion::{ControlSpec, Corners, Shapes, Spring};

use crate::m3::{self, Scheme, font::Type, shape};
pub use crate::m3_state::TARGET;
use crate::m3_state::{
    Act, Control, Radii, Semantics, Tone, Ui, act_setters, icon_glyph, label_text, overlay,
};

/// `ContainerHeight`.
pub const HEIGHT: f32 = 32.;
/// `IconSize`.
pub const ICON: f32 = 18.;
/// `LabelTextFont`.
pub const LABEL: Type = m3::font::LABEL_LARGE;
/// `FilterChipDefaults.horizontalArrangement` with both icons: start,
/// icon ↔ label, end (8 | 18 | 4 | label | 4 | 18 | 8).
pub const PADDING: f32 = 8.;
pub const GAP: f32 = 4.;
/// Without icons, 16 | label | 16.
pub const PADDING_BARE: f32 = 16.;
/// `FlatUnselectedOutlineWidth`.
pub const OUTLINE: f32 = 1.;

/// `FilterChipDefaults.shapes()`.
pub const SHAPES: Shapes = Shapes {
    // ChipsTokens.SelectedShape: CornerFull (half the height).
    checked: Corners::all(HEIGHT / 2.),
    // ChipsTokens.PressedShape: CornerSmall.
    pressed: Corners::all(shape::SMALL),
    // ChipsTokens.UnselectedShape: CornerMedium.
    ..Shapes::fixed(Corners::all(shape::MEDIUM))
};

/// A filter chip's colours (`FilterChipDefaults.filterChipColors`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub container: Tone,
    pub label: Tone,
    pub leading: Tone,
    pub trailing: Tone,
    /// Outline colour (its width is [`OUTLINE`] unselected, 0 selected).
    pub outline: Tone,
}

pub fn colors(selected: bool, s: &Scheme) -> Colors {
    if selected {
        // FlatSelectedContainerColor and Selected*Color.
        Colors {
            container: Tone::of(s.secondary_container),
            label: Tone::of(s.on_secondary_container),
            leading: Tone::of(s.on_secondary_container),
            trailing: Tone::of(s.on_secondary_container),
            outline: Tone::NONE,
        }
    } else {
        // Transparent; FlatUnselectedOutlineColor, UnselectedLabelTextColor,
        // UnselectedLeadingIconColor (Primary), UnselectedTrailingIconColor.
        Colors {
            container: Tone::NONE,
            label: Tone::of(s.on_surface_variant),
            leading: Tone::of(s.primary),
            trailing: Tone::of(s.on_surface_variant),
            outline: Tone::of(s.outline_variant),
        }
    }
}

/// One filter chip.
pub struct FilterChip {
    key: SharedString,
    label: SharedString,
    selected: bool,
    expanded: Option<bool>,
    leading: Option<&'static str>,
    trailing: Option<&'static str>,
    act: Act,
}

impl FilterChip {
    act_setters!();

    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            selected: false,
            expanded: None,
            leading: None,
            trailing: None,
            act: Act::default(),
        }
    }

    /// It opens a menu, open now or not (and is selected while it is).
    pub fn menu(mut self, open: bool) -> Self {
        self.expanded = Some(open);
        self.selected = open;
        self
    }

    pub fn leading(mut self, glyph: &'static str) -> Self {
        self.leading = Some(glyph);
        self
    }

    pub fn trailing(mut self, glyph: &'static str) -> Self {
        self.trailing = Some(glyph);
        self
    }

    pub fn render(self, ui: &Ui, cx: &mut App) -> AnyElement {
        let z = ui.zoom;
        let control = Control::new(self.key, self.act, ui, cx);
        let c = colors(self.selected, ui.scheme);
        let spec = ControlSpec {
            checked: self.selected,
            outline: if self.selected { 0. } else { OUTLINE },
            shape: Spring::FAST_SPATIAL,
            shapes: Some(SHAPES),
            ..ControlSpec::default()
        };
        let look = control.look(ui, spec);
        let radius = Radii::of(look.corners, z, HEIGHT);
        let icon = |glyph: &'static str, tone: Tone| icon_glyph(glyph, ICON, false, tone, z);
        let (start, end) = match (self.leading, self.trailing) {
            (None, None) => (PADDING_BARE, PADDING_BARE),
            // horizontalArrangement: 8 | icon | 4 | label | 14 and mirrored.
            (Some(_), None) => (PADDING, PADDING_BARE - 2.),
            (None, Some(_)) => (PADDING_BARE - 2., PADDING),
            (Some(_), Some(_)) => (PADDING, PADDING),
        };
        let over = control
            .overlay(ui, look.clone(), radius, c.label.rgb)
            .outline(look.outline, c.outline);
        let container = div()
            .relative()
            .flex_none()
            .h(px(HEIGHT * z))
            .pl(px(start * z))
            .pr(px(end * z))
            .flex()
            .items_center()
            .gap(px(GAP * z))
            .rounded_t(px(radius.top))
            .rounded_b(px(radius.bottom))
            .bg(c.container.hsla())
            .child(overlay(ui.places(), over))
            .children(self.leading.map(|glyph| icon(glyph, c.leading)))
            .child(label_text(self.label.clone(), LABEL, c.label, z))
            .children(self.trailing.map(|glyph| icon(glyph, c.trailing)));
        let mut target = control.target(z);
        if let Some(open) = self.expanded {
            target = target.aria_expanded(open);
        }
        let semantics = if self.expanded.is_some() {
            Semantics::Button
        } else {
            Semantics::Toggle(self.selected)
        };
        control
            .wire(target, ui, self.label, semantics)
            .child(container)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::m3::M3;

    #[test]
    fn filter_chip_geometry_is_the_tokens() {
        assert_eq!(HEIGHT, 32.);
        assert_eq!(TARGET, 48.);
        assert_eq!(ICON, 18.);
        assert_eq!(LABEL, m3::font::LABEL_LARGE);
        assert_eq!((PADDING, GAP, PADDING_BARE), (8., 4., 16.));
        assert_eq!(OUTLINE, 1.);
    }

    #[test]
    fn filter_chip_shapes_are_the_expressive_tokens() {
        assert_eq!(SHAPES.rest, Corners::all(12.));
        assert_eq!(SHAPES.hovered, Corners::all(12.));
        assert_eq!(SHAPES.focused, Corners::all(12.));
        assert_eq!(SHAPES.checked, Corners::all(16.));
        assert_eq!(SHAPES.pressed, Corners::all(8.));
    }

    #[test]
    fn filter_chip_colours_are_the_tokens() {
        let s = &M3;
        let off = colors(false, s);
        assert_eq!(off.container.a, 0.);
        assert_eq!(off.label, Tone::of(s.on_surface_variant));
        assert_eq!(off.leading, Tone::of(s.primary));
        assert_eq!(off.trailing, Tone::of(s.on_surface_variant));
        assert_eq!(off.outline, Tone::of(s.outline_variant));
        let on = colors(true, s);
        assert_eq!(on.container, Tone::of(s.secondary_container));
        assert_eq!(on.label, Tone::of(s.on_secondary_container));
        assert_eq!(on.leading, Tone::of(s.on_secondary_container));
        assert_eq!(on.trailing, Tone::of(s.on_secondary_container));
        assert_eq!(on.outline.a, 0.);
    }
}
