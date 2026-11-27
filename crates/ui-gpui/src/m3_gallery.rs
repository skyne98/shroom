//! The component gallery (`shroom-gpui --gallery`): every Material 3
//! component in each of its states on one screen, so a change to a
//! component is checked with one screenshot instead of driving the app.
//!
//! Hover and press come from the pointer, as in the app. Keyboard focus is
//! pinned on the samples in the "focused" column (`Ui::with_pinned_focus`):
//! only one element can hold real focus. Its chat is a scratch one in a
//! temporary directory; nothing it does reaches the user's sessions.

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, Context, FocusHandle, IntoElement, Render, SharedString, Window, div,
    prelude::*, px,
};
use shroom_ui::chat::Chat;

use crate::m3::{self, M3};
use crate::m3_button::{Button, Fab, FabColor, FabSize, IconButton, IconKind, Kind, Size, Width};
use crate::m3_chip::FilterChip;
use crate::m3_list::{ListItem, segmented_list};
use crate::m3_menu::{self, MenuItem};
use crate::m3_nav::{self, DrawerItem};
use crate::m3_state::{self, Places, Tone, TypeStyle, Ui, control_id};
use crate::m3_text_field::{self, Span, TextField};

/// Keys of the samples drawn with keyboard focus.
const FOCUSED: &[&str] = &[
    "g-button-focus",
    "g-icon-focus",
    "g-chip-focus",
    "g-list-focus",
    "g-nav-focus",
    "g-menu-focus",
];

pub struct Gallery {
    chat: Chat,
    handles: HashMap<u64, FocusHandle>,
    places: Places,
    focus: FocusHandle,
    /// Toggled samples flip on click, to see the transition.
    on: HashSet<&'static str>,
    _dir: std::path::PathBuf,
}

impl Gallery {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::load_fonts(window);
        let dir = std::env::temp_dir().join(format!("shroom-gallery-{}", std::process::id()));
        let chat = Chat::open_in(dir.clone()).with_mouse().with_pixels();
        Self {
            chat,
            handles: HashMap::new(),
            places: Places::default(),
            focus: cx.focus_handle(),
            on: [
                "g-toggle",
                "g-icon-toggle",
                "g-chip-on",
                "g-list-on",
                "g-nav-on",
            ]
            .into_iter()
            .collect(),
            _dir: dir,
        }
    }
}

/// A titled row of samples.
fn section(title: &str, samples: Vec<AnyElement>, z: f32) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(m3::space::S100 * z))
        .child(
            div()
                .type_style(m3::font::TITLE_SMALL, z)
                .text_color(Tone::of(M3.on_surface_variant).hsla())
                .child(SharedString::from(title.to_string())),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(m3::space::S200 * z))
                .children(samples),
        )
        .into_any_element()
}

/// A sample with its state written under it.
fn labelled(state: &str, sample: AnyElement, z: f32) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(m3::space::S50 * z))
        .child(sample)
        .child(
            div()
                .type_style(m3::font::LABEL_SMALL, z)
                .text_color(Tone::of(M3.outline).hsla())
                .child(SharedString::from(state.to_string())),
        )
        .into_any_element()
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The chat's frame advances its motion clock (the controls' springs).
        self.chat.frame(80, 2);
        if self.chat.animating() {
            window.request_animation_frame();
        }
        let z = 1.;
        let sink: m3_state::Sink = {
            let this = cx.entity().downgrade();
            std::rc::Rc::new(move |id, event, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.chat.control(id, event);
                    cx.notify();
                });
            })
        };
        let pinned = FOCUSED.iter().map(|key| control_id(key)).collect();
        let on = self.on.clone();
        let flip = |key: &'static str, cx: &mut Context<Self>| {
            let this = cx.entity();
            move |_: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| {
                    if !this.on.remove(key) {
                        this.on.insert(key);
                    }
                    cx.notify();
                })
            }
        };
        let flips: Vec<_> = [
            "g-toggle",
            "g-icon-toggle",
            "g-chip-on",
            "g-list-on",
            "g-nav-on",
        ]
        .into_iter()
        .map(|key| (key, flip(key, cx)))
        .collect();
        let flip_of = |key: &str| flips.iter().find(|(k, _)| *k == key).unwrap().1.clone();
        let ui = Ui::new(
            &mut self.chat,
            &mut self.handles,
            window,
            cx,
            self.places.clone(),
            sink,
            self.focus.clone(),
            z,
            &M3,
        )
        .with_pinned_focus(pinned);

        let mut buttons: Vec<AnyElement> = [
            ("filled", Kind::Filled),
            ("tonal", Kind::Tonal),
            ("elevated", Kind::Elevated),
            ("outlined", Kind::Outlined),
            ("text", Kind::Text),
        ]
        .into_iter()
        .map(|(name, kind)| {
            labelled(
                name,
                Button::new(format!("g-button-{name}"), kind, "Label")
                    .icon(m3::icon::ADD)
                    .render(&ui, cx),
                z,
            )
        })
        .collect();
        buttons.extend([
            labelled(
                "focused",
                Button::new("g-button-focus", Kind::Filled, "Label").render(&ui, cx),
                z,
            ),
            labelled(
                "disabled",
                Button::new("g-button-off", Kind::Filled, "Label")
                    .enabled(false)
                    .render(&ui, cx),
                z,
            ),
            labelled(
                "toggle (click)",
                Button::new("g-toggle", Kind::Tonal, "Toggle")
                    .toggle(on.contains("g-toggle"))
                    .on_click(flip_of("g-toggle"))
                    .render(&ui, cx),
                z,
            ),
        ]);
        let sizes = [
            ("XS", Size::XSmall),
            ("S", Size::Small),
            ("M", Size::Medium),
            ("L", Size::Large),
        ]
        .into_iter()
        .map(|(name, size)| {
            labelled(
                name,
                Button::new(format!("g-size-{name}"), Kind::Filled, "Label")
                    .size(size)
                    .render(&ui, cx),
                z,
            )
        })
        .collect();
        let mut icons: Vec<AnyElement> = [
            ("standard", IconKind::Standard),
            ("filled", IconKind::Filled),
            ("tonal", IconKind::Tonal),
            ("outlined", IconKind::Outlined),
        ]
        .into_iter()
        .map(|(name, kind)| {
            labelled(
                name,
                IconButton::new(format!("g-icon-{name}"), kind, m3::icon::SETTINGS, name)
                    .render(&ui, cx),
                z,
            )
        })
        .collect();
        icons.extend([
            labelled(
                "wide",
                IconButton::new("g-icon-wide", IconKind::Tonal, m3::icon::EDIT, "wide")
                    .width(Width::Wide)
                    .render(&ui, cx),
                z,
            ),
            labelled(
                "focused",
                IconButton::new("g-icon-focus", IconKind::Filled, m3::icon::SEARCH, "f")
                    .render(&ui, cx),
                z,
            ),
            labelled(
                "disabled",
                IconButton::new("g-icon-off", IconKind::Filled, m3::icon::SEARCH, "d")
                    .enabled(false)
                    .render(&ui, cx),
                z,
            ),
            labelled(
                "toggle (click)",
                IconButton::new("g-icon-toggle", IconKind::Standard, m3::icon::BOLT, "t")
                    .toggle(on.contains("g-icon-toggle"))
                    .fill_checked()
                    .on_click(flip_of("g-icon-toggle"))
                    .render(&ui, cx),
                z,
            ),
            labelled(
                "FAB",
                Fab::new("g-fab", m3::icon::EDIT_SQUARE, "fab").render(&ui, cx),
                z,
            ),
            labelled(
                "extended FAB",
                Fab::new("g-fab-x", m3::icon::EDIT_SQUARE, "new")
                    .extended("New session")
                    .size(FabSize::Small)
                    .color(FabColor::SecondaryContainer)
                    .render(&ui, cx),
                z,
            ),
        ]);
        let chips = vec![
            labelled(
                "rest",
                FilterChip::new("g-chip", "Filter")
                    .leading(m3::icon::SMART_TOY)
                    .render(&ui, cx),
                z,
            ),
            labelled(
                "selected (click)",
                FilterChip::new("g-chip-on", "Filter")
                    .menu(on.contains("g-chip-on"))
                    .leading(m3::icon::SMART_TOY)
                    .trailing(m3::icon::ARROW_DROP_DOWN)
                    .on_click(flip_of("g-chip-on"))
                    .render(&ui, cx),
                z,
            ),
            labelled(
                "focused",
                FilterChip::new("g-chip-focus", "Filter").render(&ui, cx),
                z,
            ),
        ];
        let list = segmented_list(
            "gallery list",
            vec![
                ListItem::new("g-list", "Headline")
                    .supporting("Supporting text")
                    .leading(m3::icon::FOLDER)
                    .trailing("100+")
                    .segmented(0, 3)
                    .container(M3.surface_container)
                    .render(&ui, cx),
                ListItem::new("g-list-on", "Selectable (click)")
                    .leading(m3::icon::HISTORY)
                    .selected(on.contains("g-list-on"))
                    .segmented(1, 3)
                    .container(M3.surface_container)
                    .on_click(flip_of("g-list-on"))
                    .render(&ui, cx),
                ListItem::new("g-list-focus", "Focused")
                    .segmented(2, 3)
                    .container(M3.surface_container)
                    .render(&ui, cx),
            ],
            &ui,
        );
        let nav = div()
            .w(px(m3_nav::SHEET_WIDTH.0 * z))
            .flex()
            .flex_col()
            .child(m3_nav::headline("Drawer", &ui))
            .child(
                DrawerItem::new("g-nav-on", "Selected (click)")
                    .icon(m3::icon::CHAT_BUBBLE)
                    .selected(on.contains("g-nav-on"))
                    .on_click(flip_of("g-nav-on"))
                    .render(&ui, cx),
            )
            .child(
                DrawerItem::new("g-nav", "Rest")
                    .icon(m3::icon::CHAT_BUBBLE)
                    .badge("3")
                    .render(&ui, cx),
            )
            .child(
                DrawerItem::new("g-nav-focus", "Focused")
                    .icon(m3::icon::CHAT_BUBBLE)
                    .render(&ui, cx),
            );
        let menu = {
            let group = m3_menu::Group::new("g-menu", 0, 1, &ui);
            let mut ruler = m3_state::Ruler::new(z, ui.device);
            let top = m3_menu::Group::open(&mut ruler);
            let items = vec![
                MenuItem::new("g-menu-a", "Item")
                    .supporting("Supporting")
                    .leading(m3::icon::SMART_TOY)
                    .trailing(m3::icon::CHEVRON_RIGHT)
                    .edges(&group, true, false)
                    .ruled(&mut ruler)
                    .render(&ui, z, cx),
                MenuItem::new("g-menu-b", "Selected")
                    .selected(true)
                    .selected_leading(m3::icon::CHECK)
                    .place(1, 3)
                    .ruled(&mut ruler)
                    .render(&ui, z, cx),
                MenuItem::new("g-menu-focus", "Focused")
                    .selected(false)
                    .place(2, 3)
                    .edges(&group, false, true)
                    .ruled(&mut ruler)
                    .render(&ui, z, cx),
            ];
            div()
                .w(px(m3_menu::width(200.) * z))
                .child(group.render(top, items, &mut ruler, &ui, z))
        };
        let field = |key: &str, label: &str, spans: Vec<Span>, focused: bool| {
            let populated = spans.iter().any(|span| *span != Span::Caret);
            TextField::new(SharedString::from(key.to_string()), label.to_string())
                .placeholder("Placeholder")
                .leading(m3_state::icon_glyph(
                    m3::icon::SEARCH,
                    m3_text_field::ICON,
                    false,
                    m3_text_field::colors(&M3).icon,
                    z,
                ))
                .rows(vec![m3_text_field::input_row(spans, focused, &ui)])
                .focused(focused)
                .populated(populated)
                .render(&ui)
                .w(px(m3_text_field::MIN_WIDTH * z))
                .into_any_element()
        };
        let fields = vec![
            labelled("empty", field("g-field-a", "Label", vec![], false), z),
            labelled(
                "focused",
                field("g-field-b", "Label", vec![Span::Caret], true),
                z,
            ),
            labelled(
                "selection",
                field(
                    "g-field-c",
                    "Label",
                    vec![
                        Span::Text("Typed ".into()),
                        Span::Selected("text".into()),
                        Span::Caret,
                    ],
                    true,
                ),
                z,
            ),
            labelled(
                "populated",
                field(
                    "g-field-d",
                    "Label",
                    vec![Span::Text("Typed text".into())],
                    false,
                ),
                z,
            ),
        ];
        ui.finish();
        div()
            .id("gallery")
            .size_full()
            .overflow_y_scroll()
            .bg(Tone::of(M3.surface).hsla())
            .text_color(Tone::of(M3.on_surface).hsla())
            .font_family(m3::SANS)
            .track_focus(&self.focus)
            .p(px(m3::space::S300 * z))
            .flex()
            .flex_col()
            .gap(px(m3::space::S300 * z))
            .child(section("Buttons", buttons, z))
            .child(section("Button sizes", sizes, z))
            .child(section("Icon buttons and FABs", icons, z))
            .child(section("Filter chips", chips, z))
            .child(section("Text fields", fields, z))
            .child(section(
                "List, drawer, menu",
                vec![
                    div().w(px(320. * z)).child(list).into_any_element(),
                    nav.into_any_element(),
                    menu.into_any_element(),
                ],
                z,
            ))
    }
}
