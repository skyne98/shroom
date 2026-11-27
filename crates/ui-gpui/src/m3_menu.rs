//! Material 3 Expressive menus, to the token values of AndroidX Compose
//! Material 3 (`StandardMenuTokens`, `SegmentedMenuTokens`) and its
//! `DropdownMenuPopup` / `DropdownMenuGroup` / `DropdownMenuItem` /
//! `MenuDefaults`: groups of items 2 apart, each group a raised
//! SurfaceContainerLow sheet.
//!
//! Motion, all from `motion`: the popup scales 0.8 ↔ 1 on fast spatial
//! and fades on fast effects, the same springs both ways (the client passes
//! the scale in as its zoom, so everything scales as Compose's
//! `graphicsLayer` does). Once the menu has been hovered, the groups not
//! under the pointer morph to their inactive 8 (fast spatial). A selected
//! item morphs to 12 and turns TertiaryContainer (fast effects), and its
//! check expands (fast spatial) and fades in (fast effects).

use gpui::{AnyElement, App, SharedString, div, prelude::*, px};
use shroom_ui::motion::{ControlSpec, Corners, Shapes, Spring};

use crate::m3::{self, Scheme, font::Type, shape};
use crate::m3_state::{
    Act, Control, Mix, Radii, Ruler, Semantics, Tone, Ui, act_setters, control_id, hoverable,
    icon_glyph, label_text, mix_tones, overlay, shadows,
};

/// `SegmentedMenuTokens.Item`: an item's minimum height.
pub const ITEM_HEIGHT: f32 = 44.;
/// `DropdownMenuItemDefaultMinWidth` / `MaxWidth`.
pub const WIDTH: (f32, f32) = (112., 280.);
/// The item's inset in its group (horizontal 4).
pub const ITEM_INSET: f32 = 4.;
/// An item with supporting text also stands 2 off the group vertically
/// (`DropdownMenuSelectableItemWithSupportTexPadding`: 4 horizontal, 2
/// vertical), so with the group's 2 it is inset 4 all round.
pub const SUPPORTING_INSET: f32 = 2.;
/// `DropdownMenuSelectableItemContentPadding` with a precision pointer:
/// start, end, top and bottom.
pub const ITEM_PADDING: (f32, f32, f32) = (16., 10., 12.);
/// `MenuArrangement` spacing with a precision pointer (`ItemBetweenSpace`).
pub const BETWEEN: f32 = 12.;
/// `LeadingIconSize`, `TrailingIconSize`.
pub const ICON: f32 = 20.;
/// The item label (code: labelLarge) and its supporting line.
pub const LABEL: Type = m3::font::LABEL_LARGE;
pub const SUPPORTING: Type = m3::font::BODY_MEDIUM;
/// `DropdownMenuGroupContentPadding` (vertical) and `GroupSpacing`.
pub const GROUP_PADDING: f32 = 2.;
pub const GROUP_GAP: f32 = 2.;
/// `DropdownMenuGroupLabel`: minimum height, start and end padding.
pub const GROUP_LABEL: (f32, f32, f32) = (32., 12., 4.);
/// `ShadowElevation`: level 2 (3 dp).
pub const ELEVATION: f32 = m3::level::L2;

/// An item's height (dp): `ITEM_HEIGHT`, or its padding around a label
/// and a supporting line.
pub fn item_height(supporting: bool) -> f32 {
    let lines = LABEL.line + if supporting { SUPPORTING.line } else { 0. };
    (2. * ITEM_PADDING.2 + lines).max(ITEM_HEIGHT)
}

/// An item's width (dp) around `text` dp of text, with or without its
/// leading and trailing icons.
pub fn item_width(text: f32, leading: bool, trailing: bool) -> f32 {
    let (start, end, _) = ITEM_PADDING;
    let icon = ICON + BETWEEN;
    start + f32::from(u8::from(leading)) * icon + text + f32::from(u8::from(trailing)) * icon + end
}

/// The menu's width (dp) for its widest item: Compose's intrinsic width
/// within `WIDTH`. Known before layout, so a menu scaling open keeps one
/// shape on every frame rather than re-measuring text at each size.
pub fn width(widest_item: f32) -> f32 {
    (widest_item + 2. * ITEM_INSET).clamp(WIDTH.0, WIDTH.1)
}

/// `MenuDefaults.groupShape(index, count)` (`MenuGroupShapes`).
pub fn group_shapes(index: usize, count: usize) -> Shapes {
    // ContainerShape CornerLarge; GroupShape and InactiveContainerShape CornerSmall.
    let (outer, inner) = (shape::LARGE, shape::SMALL);
    let shape = match (index, count) {
        (_, 1) => Corners::all(outer),
        (0, _) => Corners::new(outer, inner),
        (i, n) if i + 1 == n => Corners::new(inner, outer),
        _ => Corners::all(inner),
    };
    Shapes {
        inactive: Corners::all(inner),
        ..Shapes::fixed(shape)
    }
}

/// `MenuDefaults.itemShape(index, count)` (`MenuItemShapes`).
pub fn item_shapes(index: usize, count: usize) -> Shapes {
    // ItemShape CornerExtraSmall; ItemFirst/LastChildShape and
    // ItemSelectedShape CornerMedium.
    let (outer, inner) = (shape::MEDIUM, shape::EXTRA_SMALL);
    let shape = match (index, count) {
        (_, 1) => Corners::all(inner),
        (0, _) => Corners::new(outer, inner),
        (i, n) if i + 1 == n => Corners::new(inner, outer),
        _ => Corners::all(inner),
    };
    Shapes {
        checked: Corners::all(shape::MEDIUM),
        ..Shapes::fixed(shape)
    }
}

/// An item's colours (`MenuDefaults.selectableItemColors`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub container: Tone,
    pub text: Tone,
    pub leading: Tone,
    pub trailing: Tone,
}

mix_tones!(Colors {
    container,
    text,
    leading,
    trailing
});

pub fn colors(selected: bool, s: &Scheme) -> Colors {
    if selected {
        Colors {
            container: Tone::of(s.tertiary_container),
            text: Tone::of(s.on_tertiary_container),
            leading: Tone::of(s.on_tertiary_container),
            trailing: Tone::of(s.on_tertiary_container),
        }
    } else {
        Colors {
            container: Tone::of(s.surface_container_low),
            text: Tone::of(s.on_surface),
            leading: Tone::of(s.on_surface_variant),
            trailing: Tone::of(s.on_surface_variant),
        }
    }
}

/// One menu item.
pub struct MenuItem {
    key: SharedString,
    label: SharedString,
    supporting: Option<SharedString>,
    leading: Option<&'static str>,
    selected_leading: Option<&'static str>,
    trailing: Option<&'static str>,
    selected: Option<bool>,
    place: (usize, usize),
    /// The group's corners (dp, now) at the edges this item touches.
    edges: (Option<f32>, Option<f32>),
    /// Its height and vertical margins, measured off the menu's ruler.
    height: Option<(gpui::Pixels, gpui::Pixels, gpui::Pixels)>,
    act: Act,
}

impl MenuItem {
    act_setters!();

    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            supporting: None,
            leading: None,
            selected_leading: None,
            trailing: None,
            selected: None,
            place: (0, 1),
            edges: (None, None),
            height: None,
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

    /// The icon that expands in while it is selected (`selectedLeadingIcon`).
    pub fn selected_leading(mut self, glyph: &'static str) -> Self {
        self.selected_leading = Some(glyph);
        self
    }

    pub fn trailing(mut self, glyph: &'static str) -> Self {
        self.trailing = Some(glyph);
        self
    }

    /// One of a set of choices (a radio menu item), selected or not.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }

    /// Item `index` of `count` in its group.
    pub fn place(mut self, index: usize, count: usize) -> Self {
        self.place = (index, count);
        self
    }

    /// It sits at the top and/or bottom edge of `group`: its corners there
    /// keep inside the group's.
    pub fn edges(mut self, group: &Group, top: bool, bottom: bool) -> Self {
        let corners = group.corners();
        self.edges = (top.then_some(corners.top), bottom.then_some(corners.bottom));
        self
    }

    /// Its height off `ruler`, the menu's (see `Ruler`).
    pub fn ruled(mut self, ruler: &mut Ruler) -> Self {
        let margin = self.margin();
        let top = ruler.take(margin);
        let height = ruler.take(item_height(self.supporting.is_some()));
        self.height = Some((top, height, ruler.take(margin)));
        self
    }

    fn margin(&self) -> f32 {
        if self.supporting.is_some() {
            SUPPORTING_INSET
        } else {
            0.
        }
    }

    /// Drawn at zoom `z` (the window's, times the menu's scale).
    pub fn render(self, ui: &Ui, z: f32, cx: &mut App) -> AnyElement {
        let control = Control::new(self.key.clone(), self.act.clone(), ui, cx);
        let selected = self.selected.unwrap_or(false);
        let spec = ControlSpec {
            checked: selected,
            // The shape and the check's expansion: fast spatial; the
            // container colour: fast effects.
            shape: Spring::FAST_SPATIAL,
            colors: Spring::FAST_EFFECTS,
            // Compose: `if (selected) selectedShape else shape`; no state wins.
            shapes: Some({
                let shapes = item_shapes(self.place.0, self.place.1);
                if selected {
                    Shapes::fixed(shapes.checked)
                } else {
                    shapes
                }
            }),
            ..ControlSpec::default()
        };
        let look = control.look(ui, spec);
        let s = ui.scheme;
        let (off, on) = (colors(false, s), colors(true, s));
        let t = look.checked;
        let c = off.mix(on, t);
        let (margin, height) = match self.height {
            Some((top, height, bottom)) => ((top, bottom), height),
            None => {
                let m = px(self.margin() * z);
                ((m, m), px(item_height(self.supporting.is_some()) * z))
            }
        };
        let inset = (ITEM_INSET, GROUP_PADDING + self.margin());
        let radius = Radii::of(nested(look.corners, self.edges, inset), z, ITEM_HEIGHT);
        // The selected icon's slot grows from nothing and its icon fades in.
        let check = self
            .selected_leading
            .filter(|_| self.leading.is_none())
            .map(|glyph| {
                let grow = look.checked_shape.max(0.);
                div()
                    .flex_none()
                    .overflow_hidden()
                    .w(px((ICON + BETWEEN) * grow * z))
                    .opacity(t)
                    .child(icon_glyph(glyph, ICON, false, c.leading, z))
            });
        let text = div()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(label_text(self.label.clone(), LABEL, c.text, z))
            .children(
                self.supporting
                    .clone()
                    .map(|line| label_text(line, SUPPORTING, c.text, z)),
            );
        // State layer in the content colour; ring: FocusIndicatorColor.
        let over = control.overlay(ui, look, radius, c.text.rgb).zoom(z);
        let (start, end, y) = ITEM_PADDING;
        let item = control
            .root()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .min_w(px((WIDTH.0 - 2. * ITEM_INSET) * z))
            .max_w(px((WIDTH.1 - 2. * ITEM_INSET) * z))
            // Its height from the tokens, not its text: gpui rounds each
            // line's height, which made a scaling menu step.
            .h(height)
            .mt(margin.0)
            .mb(margin.1)
            .pl(px(start * z))
            .pr(px(end * z))
            .py(px(y * z))
            .mx(px(ITEM_INSET * z))
            .rounded_t(px(radius.top))
            .rounded_b(px(radius.bottom))
            .bg(c.container.hsla())
            .child(overlay(ui.places(), over))
            .children(self.leading.map(|glyph| {
                div()
                    .flex_none()
                    .mr(px(BETWEEN * z))
                    .child(icon_glyph(glyph, ICON, false, c.leading, z))
            }))
            .children(check)
            .child(text)
            .children(self.trailing.map(|glyph| {
                div()
                    .flex_none()
                    .ml(px(BETWEEN * z))
                    .child(icon_glyph(glyph, ICON, false, c.trailing, z))
            }));
        let semantics = match self.selected {
            Some(selected) => Semantics::Selectable {
                role: gpui::Role::MenuItemRadio,
                selected,
            },
            None => Semantics::Item(gpui::Role::MenuItem),
        };
        control
            .wire(item, ui, self.label, semantics)
            .into_any_element()
    }
}

/// A group's label (`MenuDefaults.DropdownMenuGroupLabel`).
pub fn group_label(
    text: impl Into<SharedString>,
    ruler: &mut Ruler,
    ui: &Ui,
    z: f32,
) -> AnyElement {
    let (height, start, end) = GROUP_LABEL;
    let height = ruler.take(height);
    let text: SharedString = text.into();
    div()
        .id(gpui::ElementId::Name(text.clone()))
        .flex_none()
        .flex()
        .items_center()
        .h(height)
        .pl(px((ITEM_INSET + start) * z))
        .pr(px(end * z))
        .role(gpui::Role::Heading)
        // No colour of its own: the group's content colour.
        .child(label_text(text, LABEL, Tone::of(ui.scheme.on_surface), z))
        .into_any_element()
}

/// Optical roundness (m3.material.io, Shape › Corner radius scale: "When
/// nesting rounded objects, avoid using the same corner radii … outer
/// radius − padding = inner radius"). An item's corners at a group edge are
/// the group's corners there, less its inset (`(side, vertical)`; the larger
/// where they differ), so the curves are concentric and the item never pokes
/// past the group's. Inside the group, the item keeps its own shape. With the
/// group at 16 and the inset 4 this is Compose's own first and last child
/// shape, 12. Compose clips to the group Surface where it differs (a group
/// gone inactive, a lone item); the guide's rule is what that clip
/// approximates.
pub fn nested(item: Corners, edges: (Option<f32>, Option<f32>), inset: (f32, f32)) -> Corners {
    let inset = inset.0.max(inset.1);
    let inner = |outer: Option<f32>, own: f32| outer.map_or(own, |outer| (outer - inset).max(0.));
    Corners::new(inner(edges.0, item.top), inner(edges.1, item.bottom))
}

/// A group (`DropdownMenuGroup`), `index` of `count` in its menu: its
/// shape this frame, then its sheet around the items.
pub struct Group {
    key: String,
    index: usize,
    id: u64,
    look: shroom_ui::motion::ControlLook,
}

impl Group {
    pub fn new(key: &str, index: usize, count: usize, ui: &Ui) -> Self {
        let id = group_id(key, index);
        let look = ui.look(
            id,
            ControlSpec {
                shape: Spring::FAST_SPATIAL,
                shapes: Some(group_shapes(index, count)),
                ..ControlSpec::default()
            },
        );
        Self {
            key: key.to_string(),
            index,
            id,
            look,
        }
    }

    /// The group's corners now (dp).
    pub fn corners(&self) -> Corners {
        self.look.corners
    }

    /// The sheet's top padding, off the menu's ruler: take it before laying
    /// out the children, and pass it to `render` after them.
    pub fn open(ruler: &mut Ruler) -> gpui::Pixels {
        ruler.take(GROUP_PADDING)
    }

    pub fn render(
        self,
        top: gpui::Pixels,
        children: Vec<AnyElement>,
        ruler: &mut Ruler,
        ui: &Ui,
        z: f32,
    ) -> AnyElement {
        let bottom = ruler.take(GROUP_PADDING);
        let radius = Radii::of(self.look.corners, z, f32::INFINITY);
        let sheet = div()
            .id(gpui::ElementId::Name(
                format!("{}-group-{}", self.key, self.index).into(),
            ))
            .role(gpui::Role::Group)
            .flex()
            .flex_col()
            .pt(top)
            .pb(bottom)
            .rounded_t(px(radius.top))
            .rounded_b(px(radius.bottom))
            .bg(Tone::of(ui.scheme.surface_container_low).hsla())
            .shadow(shadows(ELEVATION, z, ui.scheme.shadow))
            .children(children);
        hoverable(sheet, ui, self.id).into_any_element()
    }
}

/// Group `index`'s control id in menu `key`.
pub fn group_id(key: &str, index: usize) -> u64 {
    control_id(&format!("{key}-group-{index}"))
}

/// The popup's column of groups (`DropdownMenuPopup`), `open` its opacity;
/// the groups come with their gaps (`gap`), all off one `Ruler`.
pub fn popup(
    key: &str,
    aria: &str,
    groups: Vec<AnyElement>,
    open: f32,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(key.to_string().into()))
        .role(gpui::Role::Menu)
        .aria_label(SharedString::from(aria.to_string()))
        .flex()
        .flex_col()
        .opacity(open.clamp(0., 1.))
        .children(groups)
}

/// The gap between two groups, off the menu's ruler.
pub fn gap(ruler: &mut Ruler) -> AnyElement {
    div()
        .flex_none()
        .h(ruler.take(GROUP_GAP))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::m3::M3;

    #[test]
    fn menu_geometry_is_the_tokens() {
        assert_eq!(ITEM_HEIGHT, 44.);
        assert_eq!(WIDTH, (112., 280.));
        assert_eq!(ITEM_INSET, 4.);
        assert_eq!(SUPPORTING_INSET, 2.);
        assert_eq!(ITEM_PADDING, (16., 10., 12.));
        assert_eq!(BETWEEN, 12.);
        assert_eq!(ICON, 20.);
        assert_eq!(LABEL, m3::font::LABEL_LARGE);
        assert_eq!(SUPPORTING, m3::font::BODY_MEDIUM);
        assert_eq!((GROUP_PADDING, GROUP_GAP), (2., 2.));
        assert_eq!(GROUP_LABEL, (32., 12., 4.));
        assert_eq!(m3::level::DP[ELEVATION as usize], 3.);
    }

    #[test]
    fn menu_shapes_are_the_tokens() {
        assert_eq!(group_shapes(0, 1).rest, Corners::all(16.));
        assert_eq!(group_shapes(0, 2).rest, Corners::new(16., 8.));
        assert_eq!(group_shapes(1, 3).rest, Corners::all(8.));
        assert_eq!(group_shapes(1, 2).rest, Corners::new(8., 16.));
        assert_eq!(group_shapes(0, 2).hovered, Corners::new(16., 8.));
        for (i, n) in [(0, 1), (0, 2), (1, 2)] {
            assert_eq!(group_shapes(i, n).inactive, Corners::all(8.));
        }
        assert_eq!(item_shapes(0, 1).rest, Corners::all(4.));
        assert_eq!(item_shapes(0, 3).rest, Corners::new(12., 4.));
        assert_eq!(item_shapes(1, 3).rest, Corners::all(4.));
        assert_eq!(item_shapes(2, 3).rest, Corners::new(4., 12.));
        // Selected: 12; no hover, focus or press morph on vertical items.
        assert_eq!(item_shapes(1, 3).checked, Corners::all(12.));
        assert_eq!(item_shapes(1, 3).hovered, Corners::all(4.));
        assert_eq!(item_shapes(1, 3).pressed, Corners::all(4.));
    }

    #[test]
    fn items_keep_inside_their_group_by_optical_roundness() {
        // Outer − padding = inner: 16 − 4 = 12, Compose's first-child shape.
        let inset = (ITEM_INSET, GROUP_PADDING);
        let lone = item_shapes(0, 1).rest;
        assert_eq!(
            nested(lone, (Some(16.), Some(16.)), inset),
            Corners::all(12.)
        );
        assert_eq!(
            nested(item_shapes(0, 3).rest, (Some(16.), None), inset),
            item_shapes(0, 3).rest
        );
        assert_eq!(
            nested(item_shapes(2, 3).rest, (None, Some(16.)), inset),
            item_shapes(2, 3).rest
        );
        // A group gone inactive (8) takes its edge items down to 4.
        let selected = Corners::all(shape::MEDIUM);
        assert_eq!(
            nested(selected, (Some(8.), None), inset),
            Corners::new(4., 12.)
        );
        assert_eq!(nested(selected, (None, None), inset), selected);
        // An item with supporting text is inset 4 all round (2 + 2), so its
        // margin is even on every side and its corners are the same 12.
        assert_eq!(GROUP_PADDING + SUPPORTING_INSET, ITEM_INSET);
        let supported = (ITEM_INSET, GROUP_PADDING + SUPPORTING_INSET);
        assert_eq!(
            nested(lone, (Some(16.), Some(16.)), supported),
            Corners::all(12.)
        );
        // Concentric: the item's corner circle stays inside the group's.
        for (side, top) in [inset, supported] {
            for outer in [8., 12., 16.] {
                let r = nested(lone, (Some(outer), None), (side, top)).top;
                let centre = (side + r, top + r);
                let off = ((outer - centre.0).powi(2) + (outer - centre.1).powi(2)).sqrt();
                assert!(off + r <= outer + 1e-4, "{outer}: {r}");
            }
        }
    }

    #[test]
    fn menu_width_is_its_widest_item_within_the_tokens() {
        // 16 | 20 | 12 | text | 10, and 16 | text | 10.
        assert_eq!(item_width(100., true, false), 158.);
        assert_eq!(item_width(100., true, true), 190.);
        assert_eq!(width(item_width(100., true, true)), 198.);
        assert_eq!(width(10.), WIDTH.0);
        // 44, or 12 | 20 | 20 | 12.
        assert_eq!(item_height(false), 44.);
        assert_eq!(item_height(true), 64.);
        assert_eq!(width(1000.), WIDTH.1);
    }

    #[test]
    fn menu_colours_are_the_tokens() {
        let s = &M3;
        let off = colors(false, s);
        assert_eq!(off.container, Tone::of(s.surface_container_low));
        assert_eq!(off.text, Tone::of(s.on_surface));
        assert_eq!(off.leading, Tone::of(s.on_surface_variant));
        assert_eq!(off.trailing, Tone::of(s.on_surface_variant));
        let on = colors(true, s);
        assert_eq!(on.container, Tone::of(s.tertiary_container));
        assert_eq!(on.text, Tone::of(s.on_tertiary_container));
        assert_eq!(on.leading, Tone::of(s.on_tertiary_container));
        assert_eq!(on.trailing, Tone::of(s.on_tertiary_container));
    }
}
