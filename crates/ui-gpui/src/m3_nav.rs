//! Material 3 navigation drawer items, to the token values of AndroidX
//! Compose Material 3 (`NavigationDrawerTokens`) and its
//! `NavigationDrawerItem` / `NavigationDrawerItemDefaults`: a full-width
//! pill, 56 tall, with an optional leading icon, the label and an optional
//! trailing badge (text); the headline over a group of items.
//!
//! The drawer item is used, not the expanded wide navigation rail, because
//! shroom's sessions are a long, scrolling, growing list that needs a
//! trailing slot (the age); the rail is for three to seven fixed
//! destinations and has no trailing content.
//!
//! Selection animates on the default effects spring: the colours, and the
//! icon from outlined to filled (Material Symbols' Fill axis). Hover, focus,
//! press and the ripple come from `m3_state`.

use gpui::{AnyElement, App, SharedString, div, prelude::*, px};
use shroom_ui::motion::ControlSpec;

use crate::m3::{self, Scheme, font::Type, shape};
use crate::m3_state::{
    Act, Control, Mix, Semantics, Tone, TypeStyle, Ui, act_setters, icon_fill, label_text,
    mix_tones, overlay,
};

/// `ActiveIndicatorHeight`: the item's minimum height.
pub const HEIGHT: f32 = 56.;
/// `IconSize`.
pub const ICON: f32 = 24.;
/// The row's padding (`NavigationDrawerItem`: start 16, end 24).
pub const PADDING: (f32, f32) = (16., 24.);
/// Between icon, label and badge (`NavigationDrawerItem`'s spacers).
pub const GAP: f32 = 12.;
/// `NavigationDrawerItemDefaults.ItemPadding` (horizontal), around each item.
pub const ITEM_PADDING: f32 = 12.;
/// Between items. The drawer spec has none ("Padding between elements
/// 0dp"); the expressive update replaces the drawer with the expanded
/// navigation rail, whose item spacing token is
/// `NavigationRailCollapsedTokens.ItemVerticalSpace` (`Space50`).
pub const ITEM_SPACE: f32 = 4.;
/// `ContainerWidth` (maximum) and Compose's `MinimumDrawerWidth`.
pub const SHEET_WIDTH: (f32, f32) = (240., 360.);
/// `LabelTextFont` and `LargeBadgeLabelFont`.
pub const LABEL: Type = m3::font::LABEL_LARGE;
/// `HeadlineFont`.
pub const HEADLINE: Type = m3::font::TITLE_SMALL;

/// A drawer item's colours (`NavigationDrawerItemDefaults.colors`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub container: Tone,
    pub icon: Tone,
    pub label: Tone,
    pub badge: Tone,
    /// The Surface's content colour: state layer and ripple.
    pub state: u32,
}

mix_tones!(Colors {
    container,
    icon,
    label,
    badge
} keep { state });

pub fn colors(selected: bool, s: &Scheme) -> Colors {
    if selected {
        Colors {
            container: Tone::of(s.secondary_container),
            icon: Tone::of(s.on_secondary_container),
            label: Tone::of(s.on_secondary_container),
            badge: Tone::of(s.on_secondary_container),
            state: s.on_secondary_container,
        }
    } else {
        Colors {
            container: Tone::NONE,
            icon: Tone::of(s.on_surface_variant),
            label: Tone::of(s.on_surface_variant),
            badge: Tone::of(s.on_surface_variant),
            // Transparent container: the content colour around it.
            state: s.on_surface,
        }
    }
}

/// A group's headline (`HeadlineFont`, `HeadlineColor`), aligned with the
/// items' icons and as tall as an item.
pub fn headline(text: impl Into<SharedString>, ui: &Ui) -> AnyElement {
    let z = ui.zoom;
    let text = text.into();
    div()
        .id(gpui::ElementId::Name(text.clone()))
        .flex_none()
        .h(px(HEIGHT * z))
        .pl(px((ITEM_PADDING + PADDING.0) * z))
        .pr(px((ITEM_PADDING + PADDING.1) * z))
        .flex()
        .items_center()
        .role(gpui::Role::Heading)
        .child(label_text(
            text,
            HEADLINE,
            Tone::of(ui.scheme.on_surface_variant),
            z,
        ))
        .into_any_element()
}

/// One navigation drawer item.
pub struct DrawerItem {
    key: SharedString,
    label: SharedString,
    selected: bool,
    icon: Option<&'static str>,
    badge: Option<SharedString>,
    act: Act,
}

impl DrawerItem {
    act_setters!();

    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            selected: false,
            icon: None,
            badge: None,
            act: Act::default(),
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn icon(mut self, glyph: &'static str) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// The trailing badge: a short text (`LargeBadgeLabelFont`).
    pub fn badge(mut self, text: impl Into<SharedString>) -> Self {
        self.badge = Some(text.into());
        self
    }

    pub fn render(self, ui: &Ui, cx: &mut App) -> AnyElement {
        let z = ui.zoom;
        let control = Control::new(self.key.clone(), self.act.clone(), ui, cx);
        let spec = ControlSpec {
            checked: self.selected,
            ..ControlSpec::default()
        };
        let look = control.look(ui, spec);
        // Selection moves on the default effects spring: the colours and the
        // icon's fill, outlined to filled.
        let (off, on) = (colors(false, ui.scheme), colors(true, ui.scheme));
        let t = look.checked;
        let c = Colors {
            // The state layer's colour switches with selection.
            state: if self.selected { on.state } else { off.state },
            ..off.mix(on, t)
        };
        let radius = shape::radius(shape::FULL, HEIGHT) * z;
        let label = div()
            .flex_1()
            .min_w(px(0.))
            .one_line()
            .type_style(LABEL, z)
            .text_color(c.label.hsla())
            .child(self.label.clone());
        // State layer in the content colour; ring: FocusIndicatorColor.
        let over = control.overlay(ui, look, radius, c.state);
        let item = control
            .root()
            .relative()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(GAP * z))
            .min_h(px(HEIGHT * z))
            .pl(px(PADDING.0 * z))
            .pr(px(PADDING.1 * z))
            .rounded(px(radius))
            .bg(c.container.hsla())
            .child(overlay(ui.places(), over))
            .children(self.icon.map(|glyph| icon_fill(glyph, ICON, t, c.icon, z)))
            .child(label)
            .children(self.badge.map(|text| label_text(text, LABEL, c.badge, z)));
        let semantics = Semantics::Selectable {
            role: gpui::Role::Tab,
            selected: self.selected,
        };
        div()
            .px(px(ITEM_PADDING * z))
            .child(control.wire(item, ui, self.label, semantics))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::m3::M3;

    #[test]
    fn drawer_item_geometry_is_the_tokens() {
        // NavigationDrawerTokens and NavigationDrawerItem's layout.
        assert_eq!(HEIGHT, 56.);
        assert_eq!(ICON, 24.);
        assert_eq!(PADDING, (16., 24.));
        assert_eq!(GAP, 12.);
        assert_eq!(ITEM_PADDING, 12.);
        assert_eq!(ITEM_SPACE, m3::space::S50);
        assert_eq!(SHEET_WIDTH, (240., 360.));
        assert_eq!(LABEL, m3::font::LABEL_LARGE);
        assert_eq!(HEADLINE, m3::font::TITLE_SMALL);
        // Fully round: a pill.
        assert_eq!(shape::radius(shape::FULL, HEIGHT), 28.);
    }

    #[test]
    fn drawer_item_colours_are_the_tokens() {
        let s = &M3;
        let on = colors(true, s);
        assert_eq!(on.container, Tone::of(s.secondary_container));
        assert_eq!(on.icon, Tone::of(s.on_secondary_container));
        assert_eq!(on.label, Tone::of(s.on_secondary_container));
        assert_eq!(on.badge, Tone::of(s.on_secondary_container));
        assert_eq!(on.state, s.on_secondary_container);
        let off = colors(false, s);
        assert_eq!(off.container.a, 0.);
        assert_eq!(off.icon, Tone::of(s.on_surface_variant));
        assert_eq!(off.label, Tone::of(s.on_surface_variant));
        assert_eq!(off.badge, Tone::of(s.on_surface_variant));
        assert_eq!(off.state, s.on_surface);
    }
}
