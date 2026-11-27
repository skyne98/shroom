//! Material 3 Expressive filled text field, to the token values of AndroidX
//! Compose Material 3 (`FilledTextFieldTokens`, `TextFieldDefaults`,
//! `TextFieldImpl`) and material-web where Compose has no answer
//! (`_md-comp-filled-text-field.scss`, `_filled-field.scss`).
//!
//! Anatomy (`TextFieldDefaults.contentPaddingWithLabel`, `TextFieldLayout`):
//! a container with only its top corners rounded, an active indicator along
//! its bottom edge, a label that rests in the field and floats above the
//! text, the input rows, a placeholder shown while focused and empty, and
//! optional leading and trailing icons in 48 boxes at the edges.
//!
//! Motion (`TextFieldTransitionScope`, `IndicatorLineNode`) runs in
//! `motion::Controls::field`: the label floats on fast spatial, its colour
//! and the indicator's on fast effects, the indicator's height on fast
//! spatial, the placeholder fades on fast effects or (where Compose uses slow
//! effects, which take longer than this app's 120 ms rule) default effects.
//!
//! The field draws no focus state layer and no focus ring: its focus shows
//! in the active indicator and the label (material-web paints the state
//! layer only on `.field:not(.disabled):hover`).
//!
//! Not drawn as the spec says: BodyLarge and BodySmall's tracking (0.5 and
//! 0.4) — gpui has no letter spacing.

use gpui::{
    AnyElement, Div, SharedString, Stateful, StyledText, TextRun, div, font, prelude::*, px,
};
use shroom_ui::motion::{ControlLook, FieldSpec};

use crate::m3::{self, Scheme, font::Type, shape};
use crate::m3_state::{Overlay, Radii, Tone, TypeStyle, Ui, control_id, hoverable, overlay};

/// `TextFieldDefaults.MinHeight`.
pub const MIN_HEIGHT: f32 = 56.;
/// `TextFieldDefaults.MinWidth`.
pub const MIN_WIDTH: f32 = 280.;
/// `TextFieldPadding`: start and end, without icons.
pub const PADDING: f32 = 16.;
/// `TextFieldWithLabelVerticalPadding`: top and bottom.
pub const VERTICAL: f32 = 8.;
/// The box an icon sits in (Compose's `MinimumInteractiveSize` for the
/// leading and trailing slots, which hold icon buttons).
pub const ICON_BOX: f32 = 48.;
/// `LeadingIconSize`, `TrailingIconSize`.
pub const ICON: f32 = 24.;
/// `textFieldHorizontalIconPadding`: (48 - `IconButtonSmallTokens.IconSize`
/// 24) / 2, taken off the text's padding beside an icon (16 - 12).
pub const ICON_GAP: f32 = PADDING - (ICON_BOX - ICON) / 2.;
/// `UnfocusedIndicatorThickness`, `FocusedIndicatorThickness`
/// (`ActiveIndicatorHeight`, `FocusActiveIndicatorHeight`).
pub const INDICATOR: f32 = 1.;
pub const INDICATOR_FOCUSED: f32 = 2.;
/// `InputFont`, `LabelFont`, `InputPlaceholderFont`: BodyLarge.
pub const INPUT: Type = m3::font::BODY_LARGE;
/// The floated label: `TextFieldImpl` lerps `bodyLarge` to `bodySmall`.
pub const LABEL_SMALL: Type = m3::font::BODY_SMALL;
/// `ContainerShape`: CornerExtraSmallTop (4 on top, square at the bottom).
pub const CORNER_TOP: f32 = shape::EXTRA_SMALL;
pub const CORNER_BOTTOM: f32 = 0.;
/// `DefaultCursorThickness` (foundation, Android): 2.
pub const CARET: f32 = 2.;
/// `TextSelectionColors`: Primary at `TextSelectionBackgroundOpacity`.
pub const SELECTION_ALPHA: f32 = 0.4;
/// Where the label rests: `topPadding + minimizedLabelHalfHeight` (8 + 16/2).
pub const LABEL_REST: f32 = VERTICAL + LABEL_SMALL.line / 2.;
/// Where it floats to: `labelEndY = topPadding`.
pub const LABEL_FLOAT: f32 = VERTICAL;
/// The input's first row: `textPosition = topPadding + labelHeight`.
pub const TEXT_TOP: f32 = VERTICAL + LABEL_SMALL.line;

/// The field's height for `rows` input rows: padding, the room
/// `calculateHeight` keeps for the floated label (`minimizedLabelHalfHeight`
/// x 2), the rows, padding; at least `MinHeight`.
pub fn height(rows: usize) -> f32 {
    (TEXT_TOP + INPUT.line * rows.max(1) as f32 + VERTICAL).max(MIN_HEIGHT)
}

/// The text's (and label's) inset from the field's edge, beside an icon or
/// not.
pub fn text_inset(icon: bool) -> f32 {
    if icon { ICON_BOX + ICON_GAP } else { PADDING }
}

/// `FilledTextFieldTokens` colours, enabled (`TextFieldDefaults.colors()`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    /// `ContainerColor`: SurfaceContainerHighest (focused and not).
    pub container: Tone,
    /// `InputColor`, `FocusInputColor`: OnSurface.
    pub input: Tone,
    /// `LabelColor`, `HoverLabelColor`: OnSurfaceVariant.
    pub label: Tone,
    /// `FocusLabelColor`: Primary.
    pub label_focused: Tone,
    /// `InputPlaceholderColor`: OnSurfaceVariant.
    pub placeholder: Tone,
    /// `ActiveIndicatorColor`: OnSurfaceVariant.
    pub indicator: Tone,
    /// `HoverActiveIndicatorColor`: OnSurface (Compose's `indicatorColor`
    /// takes no hover; material-web's `:hover` rule draws it).
    pub indicator_hover: Tone,
    /// `FocusActiveIndicatorColor`: Primary.
    pub indicator_focused: Tone,
    /// `CaretColor`: Primary.
    pub caret: Tone,
    /// `TextSelectionColors.backgroundColor`.
    pub selection: Tone,
    /// `LeadingIconColor`, `TrailingIconColor`: OnSurfaceVariant.
    pub icon: Tone,
    /// The hover state layer (material-web `hover-state-layer-color`):
    /// OnSurface at `m3::HOVER`.
    pub state_layer: u32,
}

pub fn colors(s: &Scheme) -> Colors {
    Colors {
        container: Tone::of(s.surface_container_highest),
        input: Tone::of(s.on_surface),
        label: Tone::of(s.on_surface_variant),
        label_focused: Tone::of(s.primary),
        placeholder: Tone::of(s.on_surface_variant),
        indicator: Tone::of(s.on_surface_variant),
        indicator_hover: Tone::of(s.on_surface),
        indicator_focused: Tone::of(s.primary),
        caret: Tone::of(s.primary),
        selection: Tone::at(s.primary, SELECTION_ALPHA),
        icon: Tone::of(s.on_surface_variant),
        state_layer: s.on_surface,
    }
}

/// A piece of an input row.
#[derive(Clone, Debug, PartialEq)]
pub enum Span {
    Text(SharedString),
    /// Selected text: on `Colors::selection`.
    Selected(SharedString),
    /// The caret, between the glyphs around it.
    Caret,
}

/// One input row, `INPUT.line` tall in `INPUT`'s type and
/// `Colors::input`: selected text on the selection colour and, while
/// `focused`, the caret as Compose's cursor: a bar `CARET` wide (floored
/// to whole pixels, at least one) from the line's top to its bottom
/// (`getCursorRect`), centred on the glyph boundary so the text beside it
/// never moves.
pub fn input_row(spans: Vec<Span>, focused: bool, ui: &Ui) -> AnyElement {
    let z = ui.zoom;
    let c = colors(ui.scheme);
    let line = div()
        .h(px(INPUT.line * z))
        .type_style(INPUT, z)
        .whitespace_nowrap()
        .flex()
        .items_center()
        .cursor_text();
    let text = |spans: &[Span]| -> Option<StyledText> {
        let mut text = String::new();
        let mut runs = Vec::new();
        for span in spans {
            let (part, background) = match span {
                Span::Text(part) => (part, None),
                Span::Selected(part) => (part, Some(c.selection.hsla())),
                Span::Caret => continue,
            };
            if part.is_empty() {
                continue;
            }
            text.push_str(part);
            runs.push(TextRun {
                len: part.len(),
                font: gpui::Font {
                    weight: INPUT.weight,
                    ..font(m3::SANS)
                },
                color: c.input.hsla(),
                background_color: background,
                underline: None,
                strikethrough: None,
            });
        }
        (!text.is_empty()).then(|| StyledText::new(SharedString::from(text)).with_runs(runs))
    };
    let at = spans
        .iter()
        .position(|span| *span == Span::Caret)
        .filter(|_| focused);
    let Some(at) = at else {
        return line.children(text(&spans)).into_any_element();
    };
    let width = (CARET * z).floor().max(1.);
    let caret = div()
        .relative()
        .flex_none()
        .w_0()
        .h(px(INPUT.line * z))
        .child(
            div()
                .absolute()
                .top_0()
                .left(px(-width / 2.))
                .w(px(width))
                .h(px(INPUT.line * z))
                .bg(c.caret.hsla()),
        );
    line.children(text(&spans[..at]))
        .child(caret)
        .children(text(&spans[at + 1..]))
        .into_any_element()
}

/// A filled text field around its input rows (see [`input_row`]).
pub struct TextField {
    key: SharedString,
    label: SharedString,
    placeholder: Option<SharedString>,
    leading: Option<AnyElement>,
    trailing: Option<AnyElement>,
    rows: Vec<AnyElement>,
    spec: FieldSpec,
    multiline: bool,
}

impl TextField {
    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            placeholder: None,
            leading: None,
            trailing: None,
            rows: Vec::new(),
            spec: FieldSpec::default(),
            multiline: false,
        }
    }

    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = Some(text.into());
        self
    }

    /// The leading icon (an icon button), in its 48 box.
    pub fn leading(mut self, icon: impl IntoElement) -> Self {
        self.leading = Some(icon.into_any_element());
        self
    }

    /// The trailing icon (an icon button), in its 48 box.
    pub fn trailing(mut self, icon: impl IntoElement) -> Self {
        self.trailing = Some(icon.into_any_element());
        self
    }

    /// The input, one element per row.
    pub fn rows(mut self, rows: Vec<AnyElement>) -> Self {
        self.rows = rows;
        self
    }

    /// It takes more than one line (its role: a multiline text input).
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    /// Typing lands in it.
    pub fn focused(mut self, on: bool) -> Self {
        self.spec.focused = on;
        self
    }

    /// It holds text.
    pub fn populated(mut self, on: bool) -> Self {
        self.spec.populated = on;
        self
    }

    pub fn render(self, ui: &Ui) -> Stateful<Div> {
        let z = ui.zoom;
        let id = control_id(&self.key);
        let c = colors(ui.scheme);
        let look = ui.field(id, self.spec);
        let p = look.label;
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        // Only the hover layer: focus shows in the indicator and label.
        let over = Overlay::new(
            id,
            ControlLook {
                hovered: look.hovered,
                ..ControlLook::default()
            },
            Radii {
                top: CORNER_TOP * z,
                bottom: CORNER_BOTTOM * z,
            },
            c.state_layer,
            ui,
        );
        let indicator = c
            .indicator
            .mix(c.indicator_hover, look.hovered)
            .mix(c.indicator_focused, look.focus);
        let thickness = lerp(INDICATOR, INDICATOR_FOCUSED, look.indicator);
        // The label's style lerps BodyLarge to BodySmall with the float.
        let label_size = lerp(INPUT.size, LABEL_SMALL.size, p);
        let label_line = lerp(INPUT.line, LABEL_SMALL.line, p);
        let label = div()
            .absolute()
            .left_0()
            .top(px(lerp(LABEL_REST, LABEL_FLOAT, p) * z))
            .whitespace_nowrap()
            .type_style(
                Type {
                    size: label_size,
                    line: label_line,
                    ..INPUT
                },
                z,
            )
            .text_color(c.label.mix(c.label_focused, look.focus).hsla())
            .child(self.label.clone());
        let placeholder = self
            .placeholder
            .filter(|_| !self.spec.populated && look.placeholder > 0.)
            .map(|text| {
                div()
                    .absolute()
                    .left_0()
                    .top(px(TEXT_TOP * z))
                    .whitespace_nowrap()
                    .type_style(INPUT, z)
                    .text_color(c.placeholder.hsla())
                    .opacity(look.placeholder)
                    .child(text)
            });
        let icon_box = |icon: AnyElement| {
            div()
                .flex_none()
                .w(px(ICON_BOX * z))
                .flex()
                .items_center()
                .justify_center()
                .child(icon)
        };
        let middle = div()
            .relative()
            .flex_1()
            .min_w(px(0.))
            .ml(px(if self.leading.is_some() {
                ICON_GAP
            } else {
                PADDING
            } * z))
            .mr(px(if self.trailing.is_some() {
                ICON_GAP
            } else {
                PADDING
            } * z))
            .pt(px(TEXT_TOP * z))
            .pb(px(VERTICAL * z))
            .flex()
            .flex_col()
            .child(label)
            .children(placeholder)
            .children(self.rows);
        let target = div()
            .id(gpui::ElementId::Name(self.key))
            .role(if self.multiline {
                gpui::Role::MultilineTextInput
            } else {
                gpui::Role::TextInput
            })
            .aria_label(self.label)
            .relative()
            .flex_none()
            .min_w(px(MIN_WIDTH * z))
            .min_h(px(MIN_HEIGHT * z))
            .flex()
            .items_stretch()
            .rounded_t(px(CORNER_TOP * z))
            .rounded_b(px(CORNER_BOTTOM * z))
            .bg(c.container.hsla())
            .cursor_text()
            .child(overlay(ui.places(), over))
            .children(self.leading.map(icon_box))
            .child(middle)
            .children(self.trailing.map(icon_box))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(thickness * z))
                    .bg(indicator.hsla()),
            );
        hoverable(target, ui, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::m3::M3;

    #[test]
    fn geometry_is_filled_text_field_defaults() {
        assert_eq!(MIN_HEIGHT, 56.);
        assert_eq!(MIN_WIDTH, 280.);
        assert_eq!((PADDING, VERTICAL), (16., 8.));
        assert_eq!((ICON_BOX, ICON, ICON_GAP), (48., 24., 4.));
        assert_eq!((INDICATOR, INDICATOR_FOCUSED), (1., 2.));
        assert_eq!((CORNER_TOP, CORNER_BOTTOM), (4., 0.));
        // One row: 8 + 16 (floated label) + 24 + 8 = MinHeight.
        assert_eq!(height(1), 56.);
        assert_eq!(height(0), 56.);
        assert_eq!(height(3), 104.);
        // The label rests centred in a one-row field, floats to the padding.
        assert_eq!(LABEL_REST + INPUT.line / 2., MIN_HEIGHT / 2.);
        assert_eq!((LABEL_REST, LABEL_FLOAT, TEXT_TOP), (16., 8., 24.));
        assert_eq!(text_inset(true), 52.);
        assert_eq!(text_inset(false), 16.);
    }

    #[test]
    fn type_is_body_large_floating_to_body_small() {
        assert_eq!((INPUT.size, INPUT.line), (16., 24.));
        assert_eq!((LABEL_SMALL.size, LABEL_SMALL.line), (12., 16.));
        assert_eq!(CARET, 2.);
    }

    #[test]
    fn colours_are_the_filled_text_field_tokens() {
        let c = colors(&M3);
        assert_eq!(c.container, Tone::of(M3.surface_container_highest));
        assert_eq!(c.input, Tone::of(M3.on_surface));
        assert_eq!(c.label, Tone::of(M3.on_surface_variant));
        assert_eq!(c.label_focused, Tone::of(M3.primary));
        assert_eq!(c.placeholder, Tone::of(M3.on_surface_variant));
        assert_eq!(c.indicator, Tone::of(M3.on_surface_variant));
        assert_eq!(c.indicator_hover, Tone::of(M3.on_surface));
        assert_eq!(c.indicator_focused, Tone::of(M3.primary));
        assert_eq!(c.caret, Tone::of(M3.primary));
        assert_eq!(c.selection, Tone::at(M3.primary, 0.4));
        assert_eq!(c.icon, Tone::of(M3.on_surface_variant));
        assert_eq!(c.state_layer, M3.on_surface);
    }
}
