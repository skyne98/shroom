//! Material 3 Expressive list items, to the token values of AndroidX
//! Compose Material 3 (`ListTokens`) and its interactive `ListItem` /
//! `SegmentedListItem` / `ListItemDefaults`: a leading icon, a headline
//! with an optional supporting line, and trailing text.
//!
//! The shape is the expressive one: 4 at rest, 12 hovered, 16 focused,
//! pressed or selected, morphing on fast spatial (`ListItemDefaults.shapes`,
//! `rememberAnimatedShape`). In a segmented list
//! (`ListItemDefaults.segmentedShapes`) the outer corners at rest are the
//! list's 16 and the items stand 2 apart (`SegmentedGap`). Colours move
//! only with selection, on default effects; hover, focus and press show as
//! the state layer and ripple of `m3_state`.

use gpui::{AnyElement, App, SharedString, div, prelude::*, px};
use shroom_ui::motion::{ControlSpec, Corners, Shapes, Spring};

use crate::m3::{self, Scheme, font::Type, shape};
use crate::m3_state::{
    Act, Control, Mix, Radii, Semantics, Tone, TypeStyle, Ui, act_setters, icon_glyph, label_text,
    mix_tones, overlay,
};

/// `ItemOneLineContainerHeight` and `ItemTwoLineContainerHeight`.
pub const HEIGHT: [f32; 2] = [56., 72.];
/// `ItemLeadingSpace`, `ItemTrailingSpace`.
pub const PADDING: (f32, f32) = (16., 16.);
/// Top and bottom: `InteractiveListTopPadding` with a precision pointer
/// (12; `ItemTopSpace` 10 on touch). The window always has a pointer.
pub const PADDING_Y: f32 = 12.;
/// `ItemBetweenSpace`: leading ↔ text ↔ trailing.
pub const BETWEEN: f32 = 12.;
/// `ItemLeadingIconExpressiveSize`, `ItemTrailingIconExpressiveSize`.
pub const ICON: f32 = 20.;
/// `SegmentedGap` (`Space25`).
pub const SEGMENTED_GAP: f32 = 2.;
/// `ItemLabelTextFont`, `ItemSupportingTextFont`, `ItemTrailingSupportingTextFont`.
pub const HEADLINE: Type = m3::font::BODY_LARGE;
pub const SUPPORTING: Type = m3::font::BODY_MEDIUM;
pub const TRAILING: Type = m3::font::LABEL_SMALL;

/// `ListItemDefaults.shapes()` (expressive).
pub const SHAPES: Shapes = Shapes {
    // ItemHoveredContainerExpressiveShape: CornerMedium.
    hovered: Corners::all(shape::MEDIUM),
    // Item{Focused,Pressed,Selected}ContainerExpressiveShape: CornerLarge.
    focused: Corners::all(shape::LARGE),
    pressed: Corners::all(shape::LARGE),
    checked: Corners::all(shape::LARGE),
    // ItemContainerExpressiveShape: CornerExtraSmall.
    ..Shapes::fixed(Corners::all(shape::EXTRA_SMALL))
};

/// `ListItemDefaults.segmentedShapes(index, count)`: the rest shape's
/// outer corners become the list's `ContainerShape` (CornerLarge).
pub fn segmented(index: usize, count: usize) -> Shapes {
    let outer = shape::LARGE;
    let inner = SHAPES.rest;
    let rest = match (index, count) {
        (_, 1) => Corners::all(outer),
        (0, _) => Corners::new(outer, inner.bottom),
        (i, n) if i + 1 == n => Corners::new(inner.top, outer),
        _ => inner,
    };
    Shapes {
        rest,
        inactive: rest,
        ..SHAPES
    }
}

/// A list item's colours (`ListItemDefaults.colors` / `segmentedColors`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub container: Tone,
    pub headline: Tone,
    pub supporting: Tone,
    pub leading: Tone,
    pub trailing: Tone,
}

mix_tones!(Colors {
    container,
    headline,
    supporting,
    leading,
    trailing
});

/// `segmentedColors`, with the container the samples give segmented lists
/// (`containerColor = surfaceContainer`; the token is `Surface`).
pub fn colors(selected: bool, container: u32, s: &Scheme) -> Colors {
    if selected {
        // ItemSelected*: SecondaryContainer and OnSecondaryContainer.
        Colors {
            container: Tone::of(s.secondary_container),
            headline: Tone::of(s.on_secondary_container),
            supporting: Tone::of(s.on_secondary_container),
            leading: Tone::of(s.on_secondary_container),
            trailing: Tone::of(s.on_secondary_container),
        }
    } else {
        // ItemLabelTextColor OnSurface; the rest OnSurfaceVariant.
        Colors {
            container: Tone::of(container),
            headline: Tone::of(s.on_surface),
            supporting: Tone::of(s.on_surface_variant),
            leading: Tone::of(s.on_surface_variant),
            trailing: Tone::of(s.on_surface_variant),
        }
    }
}

/// One interactive list item.
pub struct ListItem {
    key: SharedString,
    headline: SharedString,
    supporting: Option<SharedString>,
    leading: Option<&'static str>,
    trailing: Option<SharedString>,
    selected: Option<bool>,
    shapes: Shapes,
    container: Option<u32>,
    act: Act,
}

impl ListItem {
    act_setters!();

    pub fn new(key: impl Into<SharedString>, headline: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            headline: headline.into(),
            supporting: None,
            leading: None,
            trailing: None,
            selected: None,
            shapes: SHAPES,
            container: None,
            act: Act::default(),
        }
    }

    pub fn supporting(mut self, text: impl Into<SharedString>) -> Self {
        self.supporting = Some(text.into());
        self
    }

    pub fn leading(mut self, glyph: &'static str) -> Self {
        self.leading = Some(glyph);
        self
    }

    /// Trailing supporting text (`ItemTrailingSupportingTextFont`).
    pub fn trailing(mut self, text: impl Into<SharedString>) -> Self {
        self.trailing = Some(text.into());
        self
    }

    /// A selectable item (a list box option), selected or not.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }

    /// Item `index` of a segmented list of `count`.
    pub fn segmented(mut self, index: usize, count: usize) -> Self {
        self.shapes = segmented(index, count);
        self
    }

    /// The container colour role (default `Surface`).
    pub fn container(mut self, role: u32) -> Self {
        self.container = Some(role);
        self
    }

    pub fn render(self, ui: &Ui, cx: &mut App) -> AnyElement {
        let z = ui.zoom;
        let control = Control::new(self.key.clone(), self.act.clone(), ui, cx);
        let selected = self.selected.unwrap_or(false);
        let spec = ControlSpec {
            checked: selected,
            shape: Spring::FAST_SPATIAL,
            shapes: Some(self.shapes),
            ..ControlSpec::default()
        };
        let look = control.look(ui, spec);
        let s = ui.scheme;
        let container = self.container.unwrap_or(s.surface);
        let (off, on) = (colors(false, container, s), colors(true, container, s));
        let t = look.checked;
        let c = off.mix(on, t);
        let height = HEIGHT[usize::from(self.supporting.is_some())];
        let radius = Radii::of(look.corners, z, height);
        let text = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(
                div()
                    .one_line()
                    .type_style(HEADLINE, z)
                    .text_color(c.headline.hsla())
                    .child(self.headline.clone()),
            )
            .children(self.supporting.clone().map(|line| {
                div()
                    .one_line()
                    .type_style(SUPPORTING, z)
                    .text_color(c.supporting.hsla())
                    .child(line)
            }));
        // State layer in the content colour; ring: FocusIndicatorColor.
        let over = control.overlay(ui, look, radius, c.headline.rgb);
        let item = control
            .root()
            .relative()
            .flex_none()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(BETWEEN * z))
            .min_h(px(height * z))
            .pl(px(PADDING.0 * z))
            .pr(px(PADDING.1 * z))
            .py(px(PADDING_Y * z))
            .rounded_t(px(radius.top))
            .rounded_b(px(radius.bottom))
            .bg(c.container.hsla())
            .child(overlay(ui.places(), over))
            .children(
                self.leading
                    .map(|glyph| icon_glyph(glyph, ICON, false, c.leading, z)),
            )
            .child(text)
            .children(
                self.trailing
                    .map(|text| label_text(text, TRAILING, c.trailing, z)),
            );
        let aria: SharedString = match &self.supporting {
            Some(line) => format!("{} {line}", self.headline).into(),
            None => self.headline.clone(),
        };
        let semantics = match self.selected {
            Some(selected) => Semantics::Selectable {
                role: gpui::Role::ListBoxOption,
                selected,
            },
            None => Semantics::Button,
        };
        control.wire(item, ui, aria, semantics).into_any_element()
    }
}

/// A segmented list: items 2 apart (`Arrangement.spacedBy(SegmentedGap)`).
pub fn segmented_list(
    label: impl Into<SharedString>,
    items: Vec<AnyElement>,
    ui: &Ui,
) -> AnyElement {
    let label = label.into();
    div()
        .id(gpui::ElementId::Name(label.clone()))
        .role(gpui::Role::List)
        .aria_label(label)
        .flex()
        .flex_col()
        .gap(px(SEGMENTED_GAP * ui.zoom))
        .children(items)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::m3::M3;

    #[test]
    fn list_item_geometry_is_the_tokens() {
        assert_eq!(HEIGHT, [56., 72.]);
        assert_eq!(PADDING, (16., 16.));
        assert_eq!(PADDING_Y, 12.);
        assert_eq!(BETWEEN, 12.);
        assert_eq!(ICON, 20.);
        assert_eq!(SEGMENTED_GAP, 2.);
        assert_eq!(HEADLINE, m3::font::BODY_LARGE);
        assert_eq!(SUPPORTING, m3::font::BODY_MEDIUM);
        assert_eq!(TRAILING, m3::font::LABEL_SMALL);
        // The padded line fits the one-line height with room to spare.
        assert!(HEADLINE.line + 2. * PADDING_Y <= HEIGHT[0]);
    }

    #[test]
    fn list_item_shapes_are_the_expressive_tokens() {
        assert_eq!(SHAPES.rest, Corners::all(4.));
        assert_eq!(SHAPES.inactive, Corners::all(4.));
        assert_eq!(SHAPES.hovered, Corners::all(12.));
        assert_eq!(SHAPES.focused, Corners::all(16.));
        assert_eq!(SHAPES.pressed, Corners::all(16.));
        assert_eq!(SHAPES.checked, Corners::all(16.));
        // segmentedShapes: only the rest shape's outer corners change.
        assert_eq!(segmented(0, 1).rest, Corners::all(16.));
        assert_eq!(segmented(0, 3).rest, Corners::new(16., 4.));
        assert_eq!(segmented(1, 3).rest, Corners::all(4.));
        assert_eq!(segmented(2, 3).rest, Corners::new(4., 16.));
        assert_eq!(segmented(2, 3).hovered, Corners::all(12.));
        assert_eq!(segmented(0, 3).pressed, Corners::all(16.));
    }

    #[test]
    fn list_item_colours_are_the_tokens() {
        let s = &M3;
        let off = colors(false, s.surface, s);
        assert_eq!(off.container, Tone::of(s.surface));
        assert_eq!(off.headline, Tone::of(s.on_surface));
        assert_eq!(off.supporting, Tone::of(s.on_surface_variant));
        assert_eq!(off.leading, Tone::of(s.on_surface_variant));
        assert_eq!(off.trailing, Tone::of(s.on_surface_variant));
        let on = colors(true, s.surface, s);
        assert_eq!(on.container, Tone::of(s.secondary_container));
        assert_eq!(on.headline, Tone::of(s.on_secondary_container));
        assert_eq!(on.leading, Tone::of(s.on_secondary_container));
    }
}
