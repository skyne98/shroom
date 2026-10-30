//! gpui client: translates gpui key/scroll events into `shroom_ui::chat::Key`
//! and paints what `Chat::frame` lays out. All behaviour lives in shroom-ui,
//! shared with the terminal client; this file is the desktop look: a session
//! sidebar, a header, prose in a proportional font, cards behind prompts and
//! code, a composer card with a send button, and real dialogs.

use std::ops::Range;
use std::time::Duration;

use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, ExternalPaths, FocusHandle, Font,
    FontStyle, FontWeight, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, Pixels, Render, ScrollDelta, ScrollWheelEvent, SharedString,
    StrikethroughStyle, StyledText, Task, TextRun, UnderlineStyle, Window, WindowBounds,
    WindowOptions, canvas, div, font, hsla, img, prelude::*, px, rgb, size,
};
use gpui_platform::application;
use shroom_ui::NIGHT;
use shroom_ui::chat::{Chat, Chrome, Cursor, Frame, Key, Mods, Outcome, Welcome};
use shroom_ui::layout::{self, Bar, Face, Paint, Panel, Row};
use shroom_ui::popup::Popup;

const FONT: &str = "JetBrainsMono Nerd Font";
/// Prose, chrome and dialogs: Inter, bundled (OFL) as "Shroom Sans".
const SANS: &str = "Shroom Sans";
/// Base metrics at zoom 1; everything scales with `Root::zoom`.
const FONT_SIZE: f32 = 14.;
/// Prose a touch larger than code, as in every editor's markdown preview.
const SANS_SIZE: f32 = 14.;
const LINE_HEIGHT: f32 = 22.;
const PAD: f32 = 16.;
const ZOOM_STEP: f32 = 1.1;
/// The reading column in mono cells (~700 px at zoom 1), as in zeron.
const COLUMN: usize = 84;
/// Chrome sizes at zoom 1, in px.
const SIDEBAR: f32 = 248.;
const HEADER: f32 = 38.;
const FOOTER: f32 = 28.;
/// Composer card padding around its text rows.
const COMPOSER_PAD: f32 = 12.;
/// The composer's toolbar row (attach, model, thinking, send), at zoom 1.
const TOOLBAR: f32 = 34.;
/// The sidebar: a shade lighter than the content, as in zeron.
const SHELL: u32 = 0x151515;
/// Hairlines between regions (white at ~8%).
const LINE: u32 = 0x252525;
/// Hovered rows and buttons (white wash at ~11%).
const HOVER: u32 = 0x232323;
/// The user's message bubble (white wash at ~8%).
const BUBBLE: u32 = 0x1d1d1d;
/// Icon glyphs (Nerd Font): sidebar, new, attach, send, stop.
const ICON_SIDEBAR: &str = "\u{f0db}";
const ICON_NEW: &str = "\u{f067}";
const ICON_ATTACH: &str = "\u{f0c6}";
const ICON_SEND: &str = "\u{f062}";
const ICON_STOP: &str = "\u{f04d}";

struct Root {
    chat: Chat,
    focus: FocusHandle,
    /// Width of one monospace cell at zoom 1, measured from the font.
    base_cell: Pixels,
    /// ctrl+= / ctrl+- / ctrl+0; saved across runs.
    zoom: f32,
    /// Where the content grid's first cell is in the window (px), set each
    /// frame: clicks map through it.
    origin: (f32, f32),
    /// The window title last set.
    title: String,
    /// Sidebar openness of the last frame (0..1).
    sidebar: f32,
    /// Last frame's layout, for clicks: sidebar width, chrome edges (px),
    /// the transcript+dock rows, where the composer's rows start (px) and
    /// their first screen row.
    hit_area: HitArea,
    /// Answering (the composer's button is stop).
    generating: bool,
    /// Proportional glyph advances, at zoom 1 in cells, by (face, bold).
    advances: Advances,
    _tick: Task<()>,
}

type Advances = std::rc::Rc<std::cell::RefCell<std::collections::HashMap<(Face, bool, char), f32>>>;
/// A screen row with its click zones.
type ZonedRow = (Row, Vec<(Range<usize>, Cursor)>);

#[derive(Clone, Copy, Default)]
struct HitArea {
    left: f32,
    top: f32,
    bottom: f32,
    /// Screen rows above the composer (transcript and dock).
    above: usize,
    composer: (f32, f32),
    first_input: usize,
    last_input: usize,
    column_start: usize,
}

/// The desktop's "reduce animations" switch (GNOME/GTK setting, also read
/// by most other desktops through it).
fn os_reduced_motion() -> bool {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "enable-animations"])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "false")
}

/// The zoom level kept in the data dir (one number).
fn load_zoom(dir: &std::path::Path) -> f32 {
    std::fs::read_to_string(dir.join("zoom"))
        .ok()
        .and_then(|text| text.trim().parse::<f32>().ok())
        .filter(|zoom| (0.5..=4.0).contains(zoom))
        .unwrap_or(1.0)
}

impl Root {
    fn cell(&self) -> Pixels {
        self.base_cell * self.zoom
    }

    fn line(&self) -> f32 {
        (LINE_HEIGHT * self.zoom).round()
    }

    fn pad(&self) -> f32 {
        PAD * self.zoom
    }

    /// Ctrl+V: images and copied files attach, text is typed.
    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let mut attached = false;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    self.chat.attach_image(image.bytes.clone(), "clipboard");
                    attached = true;
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    for path in paths.paths() {
                        self.chat.attach_file(path.clone());
                    }
                    attached = true;
                }
                ClipboardEntry::String(_) => {}
            }
        }
        if !attached && let Some(text) = item.text() {
            self.chat.paste(&text);
        }
    }

    /// Files dropped on the window: images attach, others are named in the
    /// prompt (as a terminal would paste them).
    fn on_drop(&mut self, paths: &ExternalPaths, _: &mut Window, cx: &mut Context<Self>) {
        let text: Vec<String> = paths
            .paths()
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        self.chat.paste(&text.join("\n"));
        cx.notify();
    }

    /// Change the zoom (1.0 = default), keep it for next time.
    fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.clamp(0.5, 4.0);
        let _ = std::fs::write(
            self.chat.data_dir().join("zoom"),
            format!("{:.3}", self.zoom),
        );
        self.chat
            .notice(format!("✓ zoom {:.0}% · ctrl+0 resets", self.zoom * 100.));
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = window.text_system();
        let _ = text.add_fonts(vec![
            std::borrow::Cow::Borrowed(
                include_bytes!("../fonts/ShroomSans-Regular.ttf").as_slice(),
            ),
            std::borrow::Cow::Borrowed(
                include_bytes!("../fonts/ShroomSans-SemiBold.ttf").as_slice(),
            ),
            std::borrow::Cow::Borrowed(include_bytes!("../fonts/ShroomSans-Italic.ttf").as_slice()),
        ]);
        let id = text.resolve_font(&font(FONT));
        let base_cell = text
            .advance(id, px(FONT_SIZE), 'm')
            .map(|size| size.width)
            .unwrap_or(px(FONT_SIZE * 0.6));
        // Poll agent output and run motions: one display frame (16 ms) while
        // something moves, a slow tick otherwise; idle frames cost nothing.
        let tick = cx.spawn(async move |this, cx| {
            let mut wait = Duration::from_millis(50);
            loop {
                cx.background_executor().timer(wait).await;
                let alive = this.update(cx, |root, cx| {
                    if root.chat.poll() || root.chat.animating() {
                        cx.notify();
                    }
                    root.chat.frame_interval()
                });
                match alive {
                    Ok(next) => wait = next,
                    Err(_) => break,
                }
            }
        });
        let mut chat = Chat::open()
            .with_mouse()
            .with_pixels()
            .with_column(COLUMN)
            .with_startup_prompt();
        if os_reduced_motion() {
            chat = chat.with_reduced_motion(true);
        }
        let zoom = load_zoom(chat.data_dir());
        Self {
            chat,
            focus: cx.focus_handle(),
            base_cell,
            zoom,
            origin: (0., 0.),
            title: String::new(),
            sidebar: 1.,
            generating: false,
            hit_area: HitArea::default(),
            advances: Default::default(),
            _tick: tick,
        }
    }

    /// The layout's measure for proportional prose: glyph advances of the
    /// bundled sans in cells, cached per glyph.
    fn measure(&self, window: &Window) -> layout::Measure {
        let text = window.text_system().clone();
        let cell = f32::from(self.base_cell);
        let cache = self.advances.clone();
        std::rc::Rc::new(move |run: &str, face: Face, bold: bool| {
            let mut cache = cache.borrow_mut();
            let mut total = 0.0;
            for ch in run.chars() {
                let width = *cache.entry((face, bold, ch)).or_insert_with(|| {
                    let mut f = font(SANS);
                    if bold {
                        f.weight = FontWeight::BOLD;
                    }
                    let id = text.resolve_font(&f);
                    text.advance(id, px(sans_size(face)), ch)
                        .map_or(0.6, |size| f32::from(size.width) / cell)
                });
                total += width;
            }
            total
        })
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let stroke = &event.keystroke;
        let modifiers = &stroke.modifiers;
        let mods = Mods {
            ctrl: modifiers.control,
            shift: modifiers.shift,
            alt: modifiers.alt,
        };
        let key = match stroke.key.as_str() {
            "enter" => Key::Enter,
            "tab" => Key::Tab,
            "backspace" => Key::Backspace,
            "delete" => Key::Delete,
            "escape" => Key::Esc,
            "left" => Key::Left,
            "right" => Key::Right,
            "up" => Key::Up,
            "down" => Key::Down,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "home" => Key::Home,
            "end" => Key::End,
            f if f.len() <= 3 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
                Key::F(f[1..].parse().unwrap_or(0))
            }
            _ if mods.ctrl => match stroke.key.chars().next() {
                Some(c) if stroke.key.chars().count() == 1 => Key::Char(c),
                _ => return,
            },
            // Typed text: key_char carries shift and layout (e.g. "A", "ß").
            _ => match stroke.key_char.as_deref().and_then(|s| s.chars().next()) {
                Some(c) if !c.is_control() => Key::Char(c),
                _ => return,
            },
        };
        let zoom = match key {
            Key::Char('=' | '+') if mods.ctrl => Some(self.zoom * ZOOM_STEP),
            Key::Char('-') if mods.ctrl => Some(self.zoom / ZOOM_STEP),
            Key::Char('0') if mods.ctrl => Some(1.0),
            _ => None,
        };
        if let Some(zoom) = zoom {
            self.set_zoom(zoom);
        } else if mods.ctrl && key == Key::Char('v') {
            self.paste_clipboard(cx);
        } else {
            let outcome = self.chat.key(key, mods);
            self.apply(outcome, cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn apply(&mut self, outcome: Outcome, cx: &mut Context<Self>) {
        match outcome {
            Outcome::Quit => cx.quit(),
            Outcome::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Outcome::Open(path) => cx.open_with_system(&path),
            Outcome::Redraw => {}
        }
    }

    /// Window position → grid cell of the last frame.
    /// Window position → grid cell of the last frame: transcript rows from
    /// the grid origin, prompt rows from the composer card's. None outside
    /// the chat (sidebar, header, footer).
    fn cell_at(&self, x: Pixels, y: Pixels) -> Option<(usize, usize)> {
        let (x, y) = (f32::from(x), f32::from(y));
        let area = self.hit_area;
        if x < area.left || y < area.top || y > area.bottom {
            return None;
        }
        let (cell, line) = (f32::from(self.cell()), self.line());
        if y >= area.composer.1 {
            // The card shows the row from the column's start; the chat counts
            // cells from window column 0.
            let col = ((x - area.composer.0) / cell).max(0.) as usize + area.column_start;
            let row = area.first_input + ((y - area.composer.1) / line).max(0.) as usize;
            return Some((row, col));
        }
        let col = ((x - self.origin.0) / cell).max(0.) as usize;
        let row = (((y - self.origin.1) / line).max(0.) as usize).min(area.above.saturating_sub(1));
        Some((row, col))
    }

    fn on_click(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // In the composer: the byte under the pointer in proportional text.
        if let Some((row, at)) = self.text_at(event.position.x, event.position.y, window) {
            self.chat
                .click_text(row, at, event.click_count, event.modifiers.shift);
            cx.notify();
            return;
        }
        let Some((row, col)) = self.cell_at(event.position.x, event.position.y) else {
            return;
        };
        let mods = Mods {
            ctrl: event.modifiers.control,
            shift: event.modifiers.shift,
            alt: event.modifiers.alt,
        };
        let outcome = self.chat.click(row, col, event.click_count, mods);
        self.apply(outcome, cx);
        cx.notify();
    }

    fn on_drag(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let changed = match self.text_at(event.position.x, event.position.y, window) {
            Some((row, at)) => self.chat.drag_text(row, at),
            None => match self.cell_at(event.position.x, event.position.y) {
                Some((row, col)) => self.chat.drag(row, col),
                None => false,
            },
        };
        if changed {
            cx.notify();
        }
    }

    /// Screen row and byte offset (within that row's text) under a point in
    /// the composer, measured with the sans glyph advances.
    fn text_at(&self, x: Pixels, y: Pixels, window: &Window) -> Option<(usize, usize)> {
        let (x, y) = (f32::from(x), f32::from(y));
        let area = self.hit_area;
        if y < area.composer.1 || y > area.bottom || x < area.left {
            return None;
        }
        let row = area.first_input + ((y - area.composer.1) / self.line()).max(0.) as usize;
        let row = row.min(area.last_input);
        let (start, end) = self.chat.input_span(row)?;
        let text = &self.chat.input_text()[start..end];
        let measure = self.measure(window);
        let cell = f32::from(self.cell());
        // The window's prompt rows have no marker: text starts at the edge.
        let mut left = area.composer.0;
        for (at, ch) in text.char_indices() {
            let width = measure(ch.encode_utf8(&mut [0; 4]), Face::Sans, false) * cell;
            if x < left + width / 2. {
                return Some((row, at));
            }
            left += width;
        }
        Some((row, text.len()))
    }

    /// Selecting text puts it on the primary selection (middle-click paste),
    /// like every Linux app.
    fn on_release(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.chat.release() {
            cx.write_to_primary(ClipboardItem::new_string(text));
        }
    }

    /// Middle click: paste the primary selection, like a terminal.
    fn on_middle(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_primary().and_then(|item| item.text()) {
            self.chat.paste(&text);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        // In rows, as the system reports them: a wheel notch already comes
        // as its scroll-lines setting (3), a touchpad as pixels.
        let rows = match event.delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / self.line(),
        };
        self.chat.wheel(rows);
        cx.notify();
    }
}

/// Grid metrics at the current zoom, for the free painting functions.
#[derive(Clone, Copy)]
struct Metrics {
    cell: Pixels,
    line: f32,
    pad: f32,
    zoom: f32,
}

fn color(value: u32) -> Hsla {
    rgb(value).into()
}

/// One grid row as a single shaped text with per-run styles.
/// One grid row, split into parts pinned to the cell grid: a part spans
/// exactly its columns even when a fallback font (CJK, emoji) is wider or
/// narrower than its cells, so the scrollbar and click zones never drift.
fn row(cells: Row, zones: &[(Range<usize>, Cursor)], m: Metrics) -> impl IntoElement + use<> {
    let cell = m.cell;
    let total = layout::width(&cells);
    let mut parts = Vec::new();
    let mut col = 0;
    for (range, cursor) in zones {
        if range.start > col {
            parts.push((col..range.start, Cursor::Default));
        }
        parts.push((range.clone(), *cursor));
        col = range.end;
    }
    if col < total {
        parts.push((col..total, Cursor::Default));
    }
    let line = div()
        .h(px(m.line))
        .whitespace_nowrap()
        .flex()
        .items_center();
    // Prose is proportional: its zones are counted in characters, not in
    // drawn width, so cutting it at a zone's column would tear the line.
    // Draw it whole and take the zones' cursor for the row.
    let prose = cells
        .iter()
        .any(|run| run.paint.face != Face::Mono && !run.text.trim().is_empty());
    if parts.is_empty() || prose {
        let pointer = zones.iter().any(|(_, cursor)| *cursor == Cursor::Pointer);
        let text = zones.iter().any(|(_, cursor)| *cursor == Cursor::Text);
        return line
            .when(pointer, |line| line.cursor_pointer())
            .when(!pointer && text, |line| line.cursor_text())
            .children(sized(cells, m));
    }
    let last = parts.len() - 1;
    line.children(parts.into_iter().enumerate().map(|(n, (range, cursor))| {
        div()
            .flex_none()
            .overflow_hidden()
            .w(cell * range.len() as f32)
            // The last part reaches the window edge (clicks past the text).
            .when(n == last, |part| part.flex_1())
            .when(cursor == Cursor::Pointer, |part| part.cursor_pointer())
            .when(cursor == Cursor::Text, |part| part.cursor_text())
            .flex()
            .items_center()
            .children(sized(layout::slice(&cells, range), m))
    }))
}

/// A prompt row: its runs as text, with the caret run drawn as a thin bar
/// in front of the character it sits on (a native text field's caret).
fn composer_row(cells: Row, m: Metrics) -> gpui::AnyElement {
    let caret = layout::CARET.sans();
    let at = cells.iter().position(|run| run.paint == caret);
    let line = div()
        .h(px(m.line))
        .whitespace_nowrap()
        .flex()
        .items_center()
        .cursor_text();
    let Some(at) = at else {
        return line.children(sized(cells, m)).into_any_element();
    };
    let (before, after) = cells.split_at(at);
    line.children(sized(before.to_vec(), m))
        .child(
            div()
                .flex_none()
                .w(px(2.))
                .h(px(m.line * 0.8))
                .rounded_full()
                .bg(color(NIGHT.accent)),
        )
        .children(sized(after.to_vec(), m))
        .into_any_element()
}

/// A row's runs grouped by text size (headings are larger), each group one
/// shaped text, so sizes mix on one line.
fn sized(cells: Row, m: Metrics) -> Vec<gpui::AnyElement> {
    let size = |paint: &Paint| match paint.face {
        Face::Mono => FONT_SIZE,
        face => sans_size(face),
    };
    let mut groups: Vec<(f32, Row)> = Vec::new();
    for cell in cells {
        let s = size(&cell.paint);
        match groups.last_mut() {
            Some((at, row)) if *at == s => row.push(cell),
            _ => groups.push((s, vec![cell])),
        }
    }
    if groups.is_empty() {
        groups.push((FONT_SIZE, Row::new()));
    }
    groups
        .into_iter()
        .map(|(size, row)| {
            div()
                .flex_none()
                .text_size(px(size * m.zoom))
                .child(styled(row))
                .into_any_element()
        })
        .collect()
}

fn styled(cells: Row) -> StyledText {
    let mut text = String::new();
    let mut runs = Vec::with_capacity(cells.len());
    for cell in cells {
        if cell.text.is_empty() {
            continue;
        }
        runs.push(text_run(&cell.text, cell.paint));
        text.push_str(&cell.text);
    }
    if text.is_empty() {
        text.push(' ');
        runs.push(text_run(" ", Paint::fg(NIGHT.text)));
    }
    StyledText::new(SharedString::from(text)).with_runs(runs)
}

/// Prose size per face at zoom 1.
fn sans_size(face: Face) -> f32 {
    match face {
        Face::Title(1) => 19.,
        Face::Title(2) => 16.,
        Face::Title(_) => 15.,
        _ => SANS_SIZE,
    }
}

fn text_run(text: &str, paint: Paint) -> TextRun {
    // Cards draw prompt and code backgrounds; the cells keep only marks
    // that are not panel fill (selection, inline highlights). The caret is
    // drawn as a bar by `caret_bar`, not as a filled glyph.
    let caret = paint == layout::CARET.sans();
    let background = paint
        .bg
        .filter(|bg| *bg != NIGHT.surface && !caret)
        .map(color);
    let paint = if caret { layout::TEXT.sans() } else { paint };
    TextRun {
        len: text.len(),
        font: Font {
            weight: if paint.bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            },
            style: if paint.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            ..font(if paint.face == Face::Mono { FONT } else { SANS })
        },
        color: color(paint.fg),
        background_color: background,
        underline: paint.underline.then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(color(paint.fg)),
            wavy: false,
        }),
        strikethrough: paint.strike.then(|| StrikethroughStyle {
            thickness: px(1.),
            color: Some(color(paint.fg)),
        }),
    }
}

/// One picture, placed on its cells inside the transcript box. A picture cut
/// by the top edge keeps its full size and slides up under the clip.
fn picture(shown: &shroom_ui::chat::Shown, shift: Pixels, m: Metrics) -> impl IntoElement {
    let p = &shown.picture;
    let top = px((p.row as f32 - shown.skip as f32) * m.line) + shift;
    div()
        .absolute()
        .left(m.cell * p.col as f32)
        .top(top)
        .w(m.cell * p.cols as f32)
        .h(px(p.rows as f32 * m.line))
        .bg(color(NIGHT.bg))
        .overflow_hidden()
        .opacity(shown.fade)
        .child(img(p.png.clone()).size_full().object_fit(ObjectFit::Fill))
}

/// Scrollbar overlay. Collapsed (the pointer elsewhere) it is a thin faint
/// thumb and no track, out of the way; hovered (or dragged) it springs open
/// to a full track and thumb. Its hover area is the whole cell column, so
/// it is easy to find; clicks still land on the grid underneath.
fn scrollbar(bar: Bar, m: Metrics, cx: &mut Context<Root>) -> gpui::AnyElement {
    scrollbar_at(bar, m, (m.pad, m.pad / 2.), cx)
}

/// A scrollbar on a grid whose first cell is at `origin` (px) in its parent.
fn scrollbar_at(
    bar: Bar,
    m: Metrics,
    origin: (f32, f32),
    cx: &mut Context<Root>,
) -> gpui::AnyElement {
    let (cell, line) = (m.cell, m.line);
    let rows = |range: std::ops::Range<usize>| px(range.len() as f32 * line);
    let open = bar.openness();
    let inset = px(2.);
    // 3 px collapsed, 6 px open; the track fades in with it.
    let width = 3. + 3. * open;
    let track = shroom_ui::motion::mix(NIGHT.bg, NIGHT.surface, f64::from(open));
    let thumb = shroom_ui::motion::mix(NIGHT.faint, NIGHT.dim, f64::from(open));
    let id = bar.id;
    div()
        .id(("scrollbar", id))
        .absolute()
        .left(px(origin.0) + cell * bar.col as f32)
        .top(px(origin.1 + bar.rows.start as f32 * line))
        .w(cell)
        .h(rows(bar.rows.clone()))
        .on_hover(cx.listener(move |root, hovered: &bool, _, cx| {
            if root.chat.hover_bar(id, *hovered) {
                cx.notify();
            }
        }))
        .child(
            div()
                .absolute()
                .left((cell - px(width)) / 2.)
                .top(inset)
                .w(px(width))
                .h(rows(bar.rows.clone()) - inset * 2.)
                .rounded_full()
                .bg(color(track))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px((bar.thumb.start - bar.rows.start) as f32 * line))
                        .w_full()
                        .h(rows(bar.thumb.clone()))
                        .rounded_full()
                        .bg(color(thumb)),
                ),
        )
        .into_any_element()
}

/// A dialog: a centered card over a dimmed window, title, a real text
/// field, then the items as rows with proportional names and flush-right
/// hints. Clicks inside never reach the chat; a click on the dimmed backdrop
/// closes it (the chat does that for clicks outside).
fn popup(view: Popup, hints: Vec<String>, m: Metrics, cx: &mut Context<Root>) -> impl IntoElement {
    let open = view.open.clamp(0.0, 1.0) as f32;
    let grow = (0.96 + 0.04 * view.open) as f32;
    let row_h = (m.line * 1.5).round();
    let width = (f32::from(m.cell) * view.width as f32 + 32. * m.zoom).max(360. * m.zoom) * grow;
    let (window, shift) = (view.window, view.shift);
    let selected = view.selected;
    let rows: Vec<_> = view
        .labels
        .into_iter()
        .zip(view.items)
        .enumerate()
        .skip(1)
        .map(|(n, (label, item))| {
            let (name, hint) = label.unwrap_or_default();
            let chosen = selected == Some(n);
            let line = div()
                .id(("dialog-row", n))
                .role(gpui::Role::ListBoxOption)
                .aria_label(SharedString::from(format!("{name} {hint}")))
                .aria_selected(chosen)
                .h(px(row_h))
                .ml(px(6. * m.zoom))
                .mr(px(12. * m.zoom))
                .px(px(10. * m.zoom))
                .rounded(px(6. * m.zoom))
                .flex()
                .items_center()
                .gap(px(12. * m.zoom))
                .when(chosen, |row| row.bg(color(NIGHT.select)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .font_family(SANS)
                        .text_size(px(SANS_SIZE * m.zoom))
                        .text_color(color(NIGHT.text))
                        .when(chosen, |name| name.font_weight(FontWeight::BOLD))
                        .child(SharedString::from(name)),
                )
                .child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .font_family(FONT)
                        .text_size(px(11.5 * m.zoom))
                        .text_color(color(if chosen { NIGHT.text } else { NIGHT.muted }))
                        .child(SharedString::from(hint)),
                );
            match item {
                Some(index) => line
                    .cursor_pointer()
                    .when(!chosen, |row| row.hover(|style| style.bg(color(HOVER))))
                    .on_click(cx.listener(move |root, _, _, cx| {
                        let outcome = root.chat.pick(index);
                        root.apply(outcome, cx);
                        cx.notify();
                    }))
                    .into_any_element(),
                None => line.into_any_element(),
            }
        })
        .collect();
    let field = view.field.map(|(text, placeholder)| {
        let empty = text.is_empty();
        div()
            .id("dialog-field")
            .role(gpui::Role::TextInput)
            .aria_label(SharedString::from(view.title.clone()))
            .aria_placeholder(SharedString::from(placeholder.clone()))
            .mx(px(12. * m.zoom))
            .mb(px(8. * m.zoom))
            .h(px(row_h + 4. * m.zoom))
            .px(px(10. * m.zoom))
            .rounded(px(7. * m.zoom))
            .bg(color(NIGHT.bg))
            .border_1()
            .border_color(color(NIGHT.accent))
            .flex()
            .items_center()
            .font_family(SANS)
            .text_size(px(SANS_SIZE * m.zoom))
            .when(empty, |field| {
                field.child(
                    div()
                        .w(px(2.))
                        .h(px(m.line * 0.9))
                        .mr(px(1.))
                        .bg(color(NIGHT.accent)),
                )
            })
            .child(
                div()
                    .text_color(color(if empty { NIGHT.dim } else { NIGHT.text }))
                    .child(SharedString::from(if empty { placeholder } else { text })),
            )
            .when(!empty, |field| {
                field.child(
                    div()
                        .w(px(2.))
                        .h(px(m.line * 0.9))
                        .ml(px(1.))
                        .bg(color(NIGHT.accent)),
                )
            })
    });
    let header = view.header.map(|header| {
        div()
            .ml(px(16. * m.zoom))
            .mr(px(22. * m.zoom))
            .mb(px(2. * m.zoom))
            .flex()
            .justify_end()
            .whitespace_nowrap()
            .font_family(FONT)
            .text_size(px(11.5 * m.zoom))
            .text_color(color(NIGHT.dim))
            .child(SharedString::from(header))
    });
    let has_rows = !rows.is_empty();
    let list = div()
        .id("dialog-list")
        .role(gpui::Role::ListBox)
        .h(px(window as f32 * row_h))
        .overflow_hidden()
        .child(
            div()
                .flex()
                .flex_col()
                .mt(px(-shift * row_h))
                .children(rows),
        );
    let bar = view.bar.map(|bar| {
        let top = bar.thumb.start.saturating_sub(bar.rows.start) as f32 * row_h;
        let len = bar.thumb.len() as f32 * row_h;
        div()
            .absolute()
            .right(px(3. * m.zoom))
            .top(px(top))
            .w(px(4. * m.zoom))
            .h(px(len))
            .rounded_full()
            .bg(color(NIGHT.faint))
    });
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(96. * m.zoom))
        // The window behind the dialog is blurred, then dimmed: the card is
        // the one sharp thing on screen (gpui backdrop blur, wgpu on Linux).
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    window.paint_backdrop_blur(bounds, gpui::Corners::all(px(0.)), px(5. * m.zoom));
                    window.paint_quad(gpui::fill(bounds, hsla(0., 0., 0., 0.28 * open)));
                },
            )
            .absolute()
            .inset_0(),
        )
        .child(
            div()
                .id("dialog")
                .role(gpui::Role::Dialog)
                .aria_label(SharedString::from(view.title.clone()))
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                // The box hides the window's own wheel handler: scroll here.
                .on_scroll_wheel(cx.listener(Root::on_scroll))
                .w(px(width))
                .opacity(open)
                .bg(color(NIGHT.surface))
                .border_1()
                .border_color(color(LINE))
                .rounded(px(12. * m.zoom))
                .shadow_lg()
                .py(px(8. * m.zoom))
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(16. * m.zoom))
                        .pt(px(4. * m.zoom))
                        .pb(px(10. * m.zoom))
                        .font_family(SANS)
                        .text_size(px(12.5 * m.zoom))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(NIGHT.dim))
                        .child(SharedString::from(view.title)),
                )
                .children(field)
                .children(header)
                .when(has_rows, |card| {
                    card.child(div().relative().child(list).children(bar))
                })
                // The keys that work here, on a recessed band at the foot.
                .when(has_rows && !hints.is_empty(), |card| {
                    card.child(
                        div()
                            .mt(px(6. * m.zoom))
                            .mb(px(-8. * m.zoom))
                            .px(px(16. * m.zoom))
                            .h(px(30. * m.zoom))
                            .rounded_b(px(12. * m.zoom))
                            .bg(hsla(0., 0., 0., 0.16))
                            .border_t_1()
                            .border_color(color(LINE))
                            .flex()
                            .items_center()
                            .gap(px(12. * m.zoom))
                            .text_size(px(11.5 * m.zoom))
                            .text_color(color(NIGHT.muted))
                            .children(hints.into_iter().map(SharedString::from)),
                    )
                }),
        )
}

/// The left sidebar: product mark, new-session button, the sessions with
/// their state, and the working folder at the bottom.
fn sidebar(chrome: &Chrome, m: Metrics, width: f32, cx: &mut Context<Root>) -> gpui::AnyElement {
    let z = m.zoom;
    let mark = img(gpui::ImageSource::Image(std::sync::Arc::new(
        gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            include_bytes!("../../../nix/icons/shroom-64.png").to_vec(),
        ),
    )))
    .size(px(22. * z))
    .rounded(px(5. * z));
    let new = div()
        .id("new-session")
        .role(gpui::Role::Button)
        .aria_label("New session")
        .aria_keyshortcuts("Control+N")
        .mx(px(8. * z))
        .h(px(30. * z))
        .px(px(8. * z))
        .rounded(px(7. * z))
        .flex()
        .items_center()
        .gap(px(8. * z))
        .text_color(color(NIGHT.text))
        .cursor_pointer()
        .hover(|style| style.bg(color(HOVER)))
        .on_click(cx.listener(|root, _, _, cx| {
            let outcome = root.chat.press("ctrl+n new");
            root.apply(outcome, cx);
            cx.notify();
        }))
        .child(
            div()
                .w(px(16. * z))
                .font_family(FONT)
                .text_size(px(12. * z))
                .text_color(color(NIGHT.muted))
                .child(ICON_NEW),
        )
        .child(div().flex_1().child("New session"))
        .child(
            div()
                .font_family(FONT)
                .text_size(px(11. * z))
                .text_color(color(NIGHT.faint))
                .child("ctrl+n"),
        );
    let sessions: Vec<_> = chrome
        .sessions
        .iter()
        .enumerate()
        .map(|(n, session)| {
            let id = session.id;
            let state_colour = match session.state.as_str() {
                "answering" => NIGHT.accent,
                "stopped" => NIGHT.error,
                "" => 0x3a3a3a,
                _ => NIGHT.agent,
            };
            // `4 turns · 2h ago`: the age goes on the right, like a mail list.
            let age = session
                .detail
                .rsplit(" · ")
                .next()
                .unwrap_or_default()
                .trim_end_matches(" ago")
                .to_string();
            let label = if session.state.is_empty() {
                format!("{}, {}", session.title, session.detail)
            } else {
                format!("{}, {}, {}", session.title, session.state, session.detail)
            };
            div()
                .id(("session", n))
                .role(gpui::Role::ListItem)
                .aria_label(SharedString::from(label))
                .aria_selected(session.current)
                .mx(px(8. * z))
                .h(px(30. * z))
                .px(px(8. * z))
                .rounded(px(7. * z))
                .flex()
                .items_center()
                .gap(px(8. * z))
                .cursor_pointer()
                .when(session.current, |row| {
                    row.bg(color(HOVER)).border_1().border_color(color(LINE))
                })
                .when(!session.current, |row| {
                    row.hover(|style| style.bg(color(0x1c1c1c)))
                })
                .on_click(cx.listener(move |root, _, _, cx| {
                    root.chat.open_session(id);
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_none()
                        .w(px(16. * z))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .size(px(6. * z))
                                .rounded_full()
                                .bg(color(state_colour)),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(color(if session.current {
                            NIGHT.text
                        } else {
                            NIGHT.dim
                        }))
                        .child(SharedString::from(session.title.clone())),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.5 * z))
                        .text_color(color(NIGHT.muted))
                        .child(SharedString::from(age)),
                )
                .into_any_element()
        })
        .collect();
    let section = |label: &'static str| {
        div()
            .px(px(16. * z))
            .pt(px(14. * z))
            .pb(px(6. * z))
            .text_size(px(11.5 * z))
            .text_color(color(NIGHT.muted))
            .child(label)
    };
    div()
        .id("sidebar")
        .role(gpui::Role::Navigation)
        .aria_label("Sessions")
        .flex_none()
        .w(px(width))
        .h_full()
        .overflow_hidden()
        .bg(color(SHELL))
        .border_r_1()
        .border_color(color(LINE))
        .flex()
        .flex_col()
        .font_family(SANS)
        .text_size(px(13. * z))
        .child(
            div()
                .h(px(HEADER * z))
                .px(px(14. * z))
                .flex()
                .items_center()
                .gap(px(8. * z))
                .child(mark)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(13.5 * z))
                        .text_color(color(NIGHT.text))
                        .child("shroom"),
                ),
        )
        .child(new)
        .child(section("Sessions"))
        .child(
            div()
                .id("session-list")
                .flex_1()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(1. * z))
                .children(sessions),
        )
        .child(
            div()
                .mx(px(8. * z))
                .mb(px(8. * z))
                .h(px(32. * z))
                .px(px(8. * z))
                .rounded(px(7. * z))
                .flex()
                .items_center()
                .gap(px(8. * z))
                .text_size(px(12. * z))
                .child(
                    div()
                        .flex_none()
                        .size(px(18. * z))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(FONT)
                        .text_size(px(12. * z))
                        .text_color(color(NIGHT.muted))
                        .child("\u{f07b}"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(color(NIGHT.dim))
                        .child(SharedString::from(chrome.folder.clone())),
                )
                .when_some(chrome.branch.clone(), |row, branch| {
                    row.child(
                        div()
                            .flex_none()
                            .font_family(FONT)
                            .text_size(px(11. * z))
                            .text_color(color(NIGHT.muted))
                            .child(SharedString::from(format!("\u{e725} {branch}"))),
                    )
                }),
        )
        .into_any_element()
}

/// The title bar: sidebar toggle, the session's name and folder, new
/// session. Quiet, no rule under it; the model and context live by the
/// composer.
fn header(chrome: &Chrome, m: Metrics, cx: &mut Context<Root>) -> gpui::AnyElement {
    let z = m.zoom;
    div()
        .id("header")
        .role(gpui::Role::Banner)
        .flex_none()
        .h(px(HEADER * z))
        .px(px(8. * z))
        .flex()
        .items_center()
        .gap(px(4. * z))
        .font_family(SANS)
        .child(icon_button(
            "toggle-sidebar",
            ICON_SIDEBAR,
            "Toggle sidebar",
            "ctrl+b sidebar",
            m,
            cx,
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .ml(px(4. * z))
                .flex()
                .items_baseline()
                .gap(px(8. * z))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(
                    div()
                        .flex_none()
                        .text_size(px(13. * z))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(NIGHT.text))
                        .child(SharedString::from(chrome.title.clone())),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(12. * z))
                        .text_color(color(NIGHT.muted))
                        .child(SharedString::from(chrome.folder.clone())),
                ),
        )
        .child(icon_button(
            "new-session-top",
            ICON_NEW,
            "New session",
            "ctrl+n new",
            m,
            cx,
        ))
        .into_any_element()
}

/// A square ghost button with one icon glyph that runs a key hint.
fn icon_button(
    id: &'static str,
    glyph: &'static str,
    label: &'static str,
    keys: &'static str,
    m: Metrics,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let z = m.zoom;
    let root = cx.entity();
    gpui_base::Button::new(id)
        .aria_label(label)
        .flex_none()
        .size(px(28. * z))
        .rounded(px(6. * z))
        .flex()
        .items_center()
        .justify_center()
        .font_family(FONT)
        .text_size(px(14. * z))
        .text_color(color(NIGHT.muted))
        .cursor_pointer()
        .hover(|style| style.bg(color(HOVER)).text_color(color(NIGHT.text)))
        .on_click(move |_, _, cx| {
            root.update(cx, |root, cx| {
                let outcome = root.chat.press(keys);
                root.apply(outcome, cx);
                cx.notify();
            });
        })
        .child(glyph)
}

/// The context meter under the composer: a small ring and the share used.
fn context_meter(share: f64, m: Metrics) -> impl IntoElement {
    let z = m.zoom;
    let share = share.clamp(0.0, 1.0) as f32;
    let tone = if share > 0.8 { NIGHT.code } else { NIGHT.muted };
    let size = 12. * z;
    div()
        .id("context")
        .role(gpui::Role::ProgressIndicator)
        .aria_label(SharedString::from(format!(
            "Context {:.0}% used",
            share * 100.
        )))
        .flex()
        .items_center()
        .gap(px(5. * z))
        .text_size(px(11.5 * z))
        .text_color(color(NIGHT.muted))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    let centre = bounds.center();
                    let r = bounds.size.width.min(bounds.size.height) / 2. - px(1.);
                    ring(window, centre, r, 1., color(LINE));
                    ring(window, centre, r, share, color(tone));
                },
            )
            .size(px(size)),
        )
        .child(SharedString::from(format!("{:.0}%", share * 100.)))
}

/// A ring arc from 12 o'clock, clockwise, `share` of the way round.
fn ring(window: &mut Window, centre: gpui::Point<Pixels>, r: Pixels, share: f32, tone: Hsla) {
    if share <= 0.0 {
        return;
    }
    let steps = ((64. * share).ceil() as usize).max(2);
    let mut path = gpui::PathBuilder::stroke(px(1.6));
    for n in 0..=steps {
        let angle =
            std::f32::consts::TAU * share * n as f32 / steps as f32 - std::f32::consts::FRAC_PI_2;
        let at = centre + gpui::point(r * angle.cos(), r * angle.sin());
        if n == 0 {
            path.move_to(at);
        } else {
            path.line_to(at);
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, tone);
    }
}

/// The empty session: the mark, where the agent works, the model, three
/// example tasks as buttons, and the one key to remember.
fn welcome(info: &Welcome, m: Metrics, cx: &mut Context<Root>) -> impl IntoElement {
    let z = m.zoom;
    let mark = img(gpui::ImageSource::Image(std::sync::Arc::new(
        gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            include_bytes!("../../../nix/icons/shroom-128.png").to_vec(),
        ),
    )))
    .size(px(72. * z))
    .rounded(px(16. * z));
    let fact = |label: &str, value: String, mono: bool| {
        div()
            .flex()
            .gap(px(10. * z))
            .child(
                div()
                    .w(px(84. * z))
                    .text_color(color(NIGHT.muted))
                    .child(SharedString::from(label.to_string())),
            )
            .child(
                div()
                    .when(mono, |value| value.font_family(FONT).text_size(px(13. * z)))
                    .text_color(color(NIGHT.text))
                    .child(SharedString::from(value)),
            )
    };
    let starters: Vec<_> = info
        .starters
        .iter()
        .enumerate()
        .map(|(n, text)| {
            div()
                .id(("starter", n))
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(text.clone()))
                .px(px(14. * z))
                .py(px(10. * z))
                .rounded(px(9. * z))
                .border_1()
                .border_color(color(LINE))
                .bg(color(SHELL))
                .text_color(color(NIGHT.text))
                .cursor_pointer()
                .hover(|style| style.bg(color(HOVER)).border_color(color(NIGHT.faint)))
                .on_click(cx.listener(move |root, _, _, cx| {
                    root.chat.use_starter(n);
                    cx.notify();
                }))
                .child(SharedString::from(text.clone()))
                .into_any_element()
        })
        .collect();
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .font_family(SANS)
        .text_size(px(14.5 * z))
        .child(
            div()
                .id("welcome")
                .role(gpui::Role::Group)
                .aria_label("New session")
                .w(px(480. * z))
                .flex()
                .flex_col()
                .gap(px(18. * z))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(16. * z))
                        .child(mark)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(22. * z))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(color(NIGHT.text))
                                        .child("What should we work on?"),
                                )
                                .child(
                                    div()
                                        .text_color(color(NIGHT.dim))
                                        .child("shroom reads, runs and edits code in this folder."),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4. * z))
                        .child(fact("Folder", info.folder.clone(), true))
                        .when_some(info.branch.clone(), |facts, branch| facts.child(fact("Branch", branch, true)))
                        .child(fact("Model", info.model.clone(), false))
                        .when_some(info.missing_key.clone(), |facts, key| {
                            facts.child(
                                div()
                                    .id("missing-key")
                                    .role(gpui::Role::Button)
                                    .mt(px(6. * z))
                                    .px(px(12. * z))
                                    .py(px(8. * z))
                                    .rounded(px(8. * z))
                                    .bg(color(0x2a2418))
                                    .text_color(color(NIGHT.code))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|root, _, _, cx| {
                                        let outcome = root.chat.press("ctrl+k key");
                                        root.apply(outcome, cx);
                                        cx.notify();
                                    }))
                                    .child(SharedString::from(format!(
                                        "No API key for this model ({key}). Click or press ctrl+k to enter one."
                                    ))),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8. * z))
                        .child(div().text_size(px(12.5 * z)).text_color(color(NIGHT.muted)).child("Try"))
                        .children(starters),
                )
                .child(
                    div()
                        .text_size(px(12.5 * z))
                        .text_color(color(NIGHT.muted))
                        .child("F1 lists every key. Ctrl+P runs any command."),
                ),
        )
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        let z = self.zoom;
        let m = Metrics {
            cell: self.cell(),
            line: self.line(),
            pad: self.pad(),
            zoom: z,
        };
        // Prose is measured in the bundled sans and fills the reading
        // column, as code and tables do: one edge for everything.
        layout::set_style(layout::Style {
            measure: Some(self.measure(window)),
            prose: COLUMN,
        });
        let viewport = window.viewport_size();
        // The sidebar's width glides with its openness; on a narrow window
        // it stays hidden.
        let room = f32::from(viewport.width);
        let wanted = self.chat_sidebar();
        let sidebar_w = if room < 760. * z {
            0.
        } else {
            SIDEBAR * z * wanted
        };
        let main_w = room - sidebar_w;
        let columns = ((main_w - 2. * m.pad) / f32::from(m.cell)).max(10.);
        // Header and footer are chrome; the rest is the chat's grid (the
        // composer card adds its padding around the input rows).
        let chrome_h = (HEADER + FOOTER + TOOLBAR) * z + 2. * COMPOSER_PAD * z;
        let rows = ((f32::from(viewport.height) - chrome_h - m.pad) / m.line).max(4.);
        let frame = self.chat.frame(columns as usize, rows as usize);
        let chrome = frame.chrome.clone();
        if let Some(chrome) = &chrome {
            self.sidebar = chrome.sidebar;
            self.generating = chrome.generating;
        }
        if let Some(chrome) = &chrome {
            let title = if chrome.title.is_empty() {
                "shroom".to_string()
            } else {
                format!("{} · shroom", chrome.title)
            };
            if title != self.title {
                window.set_window_title(&title);
                self.title = title;
            }
        }
        // Clicks map to grid cells from here: the transcript's first cell,
        // and the composer's first text row (its card's border and padding).
        self.origin = (sidebar_w + m.pad, HEADER * z + m.pad / 2.);
        let above = frame.rule;
        let first_input = frame.composer.start;
        let chip_rows = usize::from(frame.chips.is_some());
        let column = frame.column.clone();
        self.hit_area = HitArea {
            left: sidebar_w,
            top: HEADER * z,
            bottom: f32::from(viewport.height) - FOOTER * z,
            above,
            composer: (
                sidebar_w + m.pad + f32::from(m.cell) * column.start as f32 + 1.,
                self.origin.1
                    + (above + frame.dock.len()) as f32 * m.line
                    + 6. * z
                    + 1.
                    + COMPOSER_PAD * z * 0.75
                    + chip_rows as f32 * m.line,
            ),
            first_input,
            last_input: frame.composer.end.saturating_sub(1),
            column_start: column.start,
        };
        let head = chrome.as_ref().map(|chrome| header(chrome, m, cx));
        // The dialog covers the whole window (sidebar and title bar too).
        let mut frame = frame;
        let dialog = frame.popup.take().map(|view| {
            let hints = chrome
                .as_ref()
                .map(|chrome| chrome.hints.clone())
                .unwrap_or_default();
            popup(view, hints, m, cx).into_any_element()
        });
        let body = chat_body(frame, chrome.clone(), m, columns as usize, cx);
        let side = (sidebar_w > 1.)
            .then(|| {
                chrome
                    .as_ref()
                    .map(|chrome| sidebar(chrome, m, sidebar_w, cx))
            })
            .flatten();
        let main = div()
            .id("main")
            .role(gpui::Role::Main)
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .bg(color(NIGHT.bg))
            .children(head)
            .child(body);
        div()
            .size_full()
            .relative()
            .flex()
            .bg(color(SHELL))
            .font_family(SANS)
            .text_size(px(SANS_SIZE * z))
            .line_height(px(m.line))
            .text_color(color(NIGHT.text))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_click))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_middle))
            .on_mouse_move(cx.listener(Self::on_drag))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_release))
            .on_drop(cx.listener(Self::on_drop))
            .children(side)
            .child(main)
            .children(dialog)
    }
}

impl Root {
    fn chat_sidebar(&mut self) -> f32 {
        // Last frame's openness (the frame computes the glide).
        self.sidebar
    }
}

/// The chat area: transcript rows over their cards, the dock, then the
/// composer card; pictures and scrollbars on top; dialogs over everything.
fn chat_body(
    frame: Frame,
    chrome: Option<Chrome>,
    m: Metrics,
    window_cols: usize,
    cx: &mut Context<Root>,
) -> gpui::AnyElement {
    let z = m.zoom;
    let Frame {
        cards,
        welcome: empty,
        rows: all,
        cursors,
        viewport,
        shift,
        above,
        bars,
        popup: dialog,
        pictures,
        dock,
        composer,
        chips,
        content,
        column,
        ..
    } = frame;
    let mut rows: Vec<_> = all.into_iter().zip(cursors).collect();
    let mut below = rows.split_off(viewport.min(rows.len()));
    let shift_px = px(shift * m.line);
    let pictures = pictures
        .iter()
        .filter(|shown| shown.picture.row < viewport)
        .map(|shown| picture(shown, shift_px, m))
        .collect::<Vec<_>>();
    let cards = cards.into_iter().map(|card| {
        let top = card.top as f32 * m.line + f32::from(shift_px);
        let (bg, radius) = match card.kind {
            Panel::Prompt => (BUBBLE, 16.),
            Panel::Code => (0x141414, 8.),
        };
        // The bubble gets a little more air than a code card.
        let (padx, pady) = match card.kind {
            Panel::Prompt => (14., 8.),
            Panel::Code => (10., 6.),
        };
        div()
            .absolute()
            .left(m.cell * card.cols.start as f32 - px(padx * z))
            .top(px(top - pady * z))
            .w(m.cell * card.cols.len() as f32 + px(2. * padx * z))
            .h(px(card.rows as f32 * m.line + 2. * pady * z))
            .rounded(px(radius * z))
            .bg(color(bg))
            .when(card.kind == Panel::Code, |code| {
                code.border_1().border_color(color(LINE))
            })
            .opacity(card.fade * content)
    });
    let transcript = div()
        .id("transcript")
        .role(gpui::Role::Log)
        .aria_label("Conversation")
        .relative()
        .h(px(viewport as f32 * m.line))
        .overflow_hidden()
        .children(cards)
        .child(
            div()
                .flex()
                .flex_col()
                .mt(shift_px - px(m.line))
                .child(row(above.unwrap_or_default(), &[], m))
                .children(rows.drain(..).map(|(cells, zones)| row(cells, &zones, m))),
        )
        .children(pictures)
        .when_some(empty.filter(|_| dialog.is_none()), |area, info| {
            area.child(welcome(&info, m, cx))
        });
    // Below the transcript: dock rows, then (skipping the old rule) the
    // chips and the composer card; the old status row is the footer now.
    let first = viewport;
    let take = |range: Range<usize>, below: &mut Vec<ZonedRow>| {
        range
            .filter_map(|at| below.get(at - first).cloned())
            .map(|(cells, zones)| row(cells, &zones, m))
            .collect::<Vec<_>>()
    };
    let dock_rows = take(dock.clone(), &mut below);
    // Chips and prompt rows: their cells from the column's start, inside
    // the card (the grid centred them with leading spaces).
    let inside = |range: Range<usize>, below: &Vec<ZonedRow>| {
        range
            .filter_map(|at| below.get(at - first).cloned())
            .map(|(cells, _)| composer_row(layout::slice(&cells, column.start..usize::MAX), m))
            .collect::<Vec<_>>()
    };
    let chip_rows = chips
        .map(|at| inside(at..at + 1, &below))
        .unwrap_or_default();
    let input_rows = inside(composer.clone(), &below);

    // The composer sits under the reading column, as wide as it.
    let column_left = m.cell * column.start as f32;
    let column_w = m.cell * column.len() as f32;
    let generating = chrome.as_ref().is_some_and(|chrome| chrome.generating);
    let can_send = chrome.as_ref().is_some_and(|chrome| chrome.can_send);
    let root = cx.entity();
    let attach = gpui_base::Button::new("attach")
        .aria_label("Attach files")
        .flex_none()
        .size(px(28. * z))
        .rounded(px(14. * z))
        .flex()
        .items_center()
        .justify_center()
        .font_family(FONT)
        .text_size(px(14. * z))
        .text_color(color(NIGHT.muted))
        .cursor_pointer()
        .hover(|style| style.bg(color(HOVER)).text_color(color(NIGHT.text)))
        .on_click(move |_, _, cx| {
            let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: Some("Attach".into()),
            });
            let root = root.clone();
            cx.spawn(async move |cx| {
                if let Ok(Ok(Some(paths))) = picked.await {
                    let text: Vec<String> = paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect();
                    let _ = root.update(cx, |root, cx| {
                        root.chat.paste(&text.join("\n"));
                        cx.notify();
                    });
                }
            })
            .detach();
        })
        .child(ICON_ATTACH);
    let ghost = |id: &'static str, label: String, keys: &'static str, cx: &mut Context<Root>| {
        let root = cx.entity();
        gpui_base::Button::new(id)
            .aria_label(SharedString::from(label.clone()))
            .flex_none()
            .h(px(28. * z))
            .px(px(8. * z))
            .rounded(px(7. * z))
            .flex()
            .items_center()
            .text_size(px(12.5 * z))
            .text_color(color(NIGHT.dim))
            .cursor_pointer()
            .hover(|style| style.bg(color(HOVER)).text_color(color(NIGHT.text)))
            .on_click(move |_, _, cx| {
                root.update(cx, |root, cx| {
                    let outcome = root.chat.press(keys);
                    root.apply(outcome, cx);
                    cx.notify();
                });
            })
            .child(SharedString::from(label))
    };
    let model = chrome
        .as_ref()
        .map(|chrome| ghost("composer-model", chrome.model.clone(), "ctrl+l model", cx));
    let thinking = chrome
        .as_ref()
        .and_then(|chrome| chrome.thinking.clone())
        .map(|label| ghost("composer-thinking", label, "shift+tab thinking", cx));
    let toolbar = div()
        .h(px((TOOLBAR - 6.) * z))
        .flex()
        .items_center()
        .gap(px(2. * z))
        .child(attach)
        .child(div().flex_1())
        .children(model)
        .children(thinking)
        .child(div().w(px(4. * z)))
        .child(send_button(generating, can_send, m, cx));
    let composer_card = div()
        .id("composer")
        .role(gpui::Role::MultilineTextInput)
        .aria_label("Message")
        .ml(column_left - px(COMPOSER_PAD * z))
        .w(column_w + px(2. * COMPOSER_PAD * z))
        .mt(px(6. * z))
        .px(px(COMPOSER_PAD * z))
        .pt(px(COMPOSER_PAD * z * 0.75))
        .pb(px(6. * z))
        .rounded(px(18. * z))
        .bg(color(0x171717))
        .border_1()
        .border_color(color(LINE))
        .shadow_md()
        .flex()
        .flex_col()
        .gap(px(6. * z))
        .child(
            div()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .children(chip_rows)
                .children(input_rows),
        )
        .child(toolbar);
    // Under the composer: a notice on the left, the context ring on the right.
    let notice = chrome
        .as_ref()
        .and_then(|chrome| chrome.notice.clone())
        .map(|(text, good, fade)| {
            div()
                .id("notice")
                .role(gpui::Role::Status)
                .aria_label(SharedString::from(text.clone()))
                .opacity(fade)
                .flex()
                .items_center()
                .gap(px(6. * z))
                .text_color(color(if good { NIGHT.accent } else { NIGHT.code }))
                .when(good, |line| line.child("✓"))
                .child(SharedString::from(text))
        });
    let context = chrome
        .as_ref()
        .and_then(|chrome| chrome.context)
        .map(|share| context_meter(share, m));
    let status = div()
        .flex_none()
        .ml(column_left)
        .w(column_w)
        .h(px(FOOTER * z))
        .flex()
        .items_center()
        .justify_between()
        .text_size(px(12. * z))
        .text_color(color(NIGHT.muted))
        .child(div().min_w(px(0.)).children(notice))
        .children(context);
    let mut bars_el = Vec::new();
    for mut bar in bars
        .into_iter()
        .filter(|bar| bar.id == layout::TRANSCRIPT_BAR)
    {
        // At the window's edge, not beside the text (where it read as a rule).
        bar.col = window_cols.saturating_sub(1);
        bars_el.push(scrollbar(bar, m, cx));
    }
    div()
        .flex_1()
        .min_h(px(0.))
        .relative()
        .px(px(m.pad))
        .pt(px(m.pad / 2.))
        .flex()
        .flex_col()
        .child(transcript)
        .child(div().flex().flex_col().children(dock_rows))
        .child(composer_card)
        .child(status)
        .children(bars_el)
        .into_any_element()
}

/// Send (or stop, while answering): a round light plate, dim when there is
/// nothing to send.
fn send_button(
    generating: bool,
    can_send: bool,
    m: Metrics,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let z = m.zoom;
    let (label, glyph) = if generating {
        ("Stop", ICON_STOP)
    } else {
        ("Send", ICON_SEND)
    };
    let live = generating || can_send;
    let root = cx.entity();
    // gpui-base's button owns pointer, Enter/Space and focus behaviour; the
    // look stays shroom's.
    gpui_base::Button::new("send")
        .aria_label(label)
        .aria_keyshortcuts(if generating { "Escape" } else { "Enter" })
        .disabled(!live)
        .flex_none()
        .size(px(28. * z))
        .rounded_full()
        .bg(color(if live { NIGHT.text } else { 0x2a2a2a }))
        .flex()
        .items_center()
        .justify_center()
        .font_family(FONT)
        .text_size(px(13. * z))
        .text_color(color(if live { NIGHT.bg } else { NIGHT.muted }))
        .when(live, |button| button.cursor_pointer())
        .on_click(move |_, _, cx| {
            root.update(cx, |root, cx| {
                if generating {
                    root.chat.stop();
                } else {
                    let outcome = root.chat.key(Key::Enter, Mods::default());
                    root.apply(outcome, cx);
                }
                cx.notify();
            });
        })
        .child(glyph)
}

fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("shroom {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(900.), px(640.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // Matches the launcher entry (StartupWMClass) so docks and
                // window lists show it as shroom.
                app_id: Some("shroom".into()),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Root::new(window, cx)),
        );
        if let Err(err) = opened {
            eprintln!("shroom: cannot open a window: {err}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}
