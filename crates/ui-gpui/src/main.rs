//! gpui client: translates gpui key/scroll events into `shroom_ui::chat::Key`
//! and paints what `Chat::frame` lays out. All behaviour lives in shroom-ui,
//! shared with the terminal client; this file is the desktop look: a session
//! sidebar, a header, prose in a proportional font, cards behind prompts and
//! code, a composer card with a send button, and real dialogs.

#[allow(dead_code)] // the full token set; not every role is drawn yet
mod m3;
#[allow(dead_code)] // the full component set; not every variant is used yet
mod m3_button;
mod m3_chip;
mod m3_gallery;
mod m3_list;
mod m3_menu;
mod m3_nav;
#[allow(dead_code)]
mod m3_state;
mod m3_text_field;

use m3::{M3, shape};
use m3_button::{Fab, IconButton, IconKind, Size};
use m3_nav::DrawerItem;
use m3_state::{TypeStyle, Ui};
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
// The sidebar is a navigation drawer sheet (`NavigationDrawerTokens`).
const _: () = assert!(SIDEBAR >= m3_nav::SHEET_WIDTH.0 && SIDEBAR <= m3_nav::SHEET_WIDTH.1);
/// The small top app bar (`AppBarSmallTokens.ContainerHeight`).
const HEADER: f32 = 64.;
/// The row under the composer (model chip, notice, context ring): as tall
/// as the chip's 48 target.
const STATUS: f32 = m3_chip::TARGET;
/// Between the transcript and the composer. Layout spacing, which the text
/// field spec has no answer for: one step of the 8 dp grid (`space::S100`).
const COMPOSER_GAP: f32 = m3::space::S100;
/// The window behind the content pane: the navigation drawer's container.
const SHELL: u32 = M3.surface_container;
/// The content pane (Material 3 surface).
const PANE: u32 = M3.surface;
/// The user's message bubble: a tonal container.
const BUBBLE: u32 = M3.secondary_container;
/// Code cards sit a step below the pane.
const CODE_CARD: u32 = M3.surface_container_lowest;

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
    /// Last frame's transcript rows as drawn, for mapping the pointer to a
    /// cell in proportional text.
    shown: Vec<Row>,
    /// The reading column last frame (the composer's icons stand beside it).
    column: Range<usize>,
    /// When the last frame was laid out (the tick's stall guard).
    rendered: std::time::Instant,
    /// Proportional glyph advances, at zoom 1 in cells, by (face, bold).
    advances: Advances,
    /// Focus handles of the drawn Material components, by control id.
    handles: std::collections::HashMap<u64, FocusHandle>,
    /// Their container bounds last frame (presses ripple from the pointer).
    places: m3_state::Places,
    _tick: Task<()>,
}

type Advances =
    std::rc::Rc<std::cell::RefCell<std::collections::HashMap<(Face, bool, bool, char), f32>>>;
/// A screen row with its click zones.
type ZonedRow = (Row, Vec<(Range<usize>, Cursor)>);

#[derive(Clone, Copy, Default)]
struct HitArea {
    left: f32,
    top: f32,
    bottom: f32,
    /// Screen rows above the composer (transcript and dock).
    above: usize,
    /// The composer's text field's top edge, and its first text row.
    field_top: f32,
    /// How far the transcript is drawn below its cells (px, a scroll glide).
    shift: f32,
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

    /// One input row of the composer: the text field's BodyLarge line.
    fn input_line(&self) -> f32 {
        m3_text_field::INPUT.line * self.zoom
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
        load_fonts(window);
        let text = window.text_system();
        let id = text.resolve_font(&font(FONT));
        let base_cell = text
            .advance(id, px(FONT_SIZE), 'm')
            .map(|size| size.width)
            .unwrap_or(px(FONT_SIZE * 0.6));
        // Poll agent output: every 16 ms while something moves, a slow tick
        // otherwise. Motion itself runs on the display's frames (`render`
        // asks for the next one while anything moves); a timer frame lands
        // 14 or 21 ms apart against a 60 Hz display, which reads as judder.
        // The timer only restarts motion that lost its frame chain.
        let tick = cx.spawn(async move |this, cx| {
            let mut wait = Duration::from_millis(50);
            loop {
                cx.background_executor().timer(wait).await;
                let alive = this.update(cx, |root, cx| {
                    let stalled = root.chat.animating()
                        && root.rendered.elapsed() > Duration::from_millis(50);
                    if root.chat.poll() || stalled {
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
        let mut chat = Chat::open();
        if std::env::args().any(|arg| arg == "--resume") {
            chat = chat.resumed();
        }
        let mut chat = chat
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
            shown: Vec::new(),
            column: 0..0,
            rendered: std::time::Instant::now(),
            hit_area: HitArea::default(),
            advances: Default::default(),
            handles: Default::default(),
            places: Default::default(),
            _tick: tick,
        }
    }

    /// The layout's measure for proportional prose: glyph advances of the
    /// bundled sans in cells, cached per glyph.
    fn measure(&self, window: &Window) -> layout::Measure {
        let text = window.text_system().clone();
        let cell = f32::from(self.base_cell);
        let cache = self.advances.clone();
        std::rc::Rc::new(move |run: &str, paint: Paint| {
            let (face, bold, italic) = (paint.face, paint.bold, paint.italic);
            let mut cache = cache.borrow_mut();
            let mut total = 0.0;
            for ch in run.chars() {
                let width = *cache.entry((face, bold, italic, ch)).or_insert_with(|| {
                    let mut f = font(SANS);
                    if bold {
                        f.weight = FontWeight::BOLD;
                    }
                    if italic {
                        f.style = FontStyle::Italic;
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

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let stroke = &event.keystroke;
        let modifiers = &stroke.modifiers;
        // F6 moves keyboard focus through the window's components (Tab
        // belongs to the composer); on a component, Tab moves on, Escape
        // goes back to typing, Enter and Space are the component's, and
        // anything else is typing.
        let on_component = !self.focus.is_focused(window);
        match stroke.key.as_str() {
            "f6" | "tab" if on_component || stroke.key == "f6" => {
                if modifiers.shift {
                    window.focus_prev(cx);
                } else {
                    window.focus_next(cx);
                }
                cx.stop_propagation();
                cx.notify();
                return;
            }
            "escape" if on_component => {
                window.focus(&self.focus, cx);
                cx.stop_propagation();
                cx.notify();
                return;
            }
            "enter" | "space" if on_component && !modifiers.modified() => return,
            _ if on_component => window.focus(&self.focus, cx),
            _ => {}
        }
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
    /// The cell under `x` in a transcript row: prose is proportional, so
    /// walk its glyphs with their drawn widths.
    fn col_at(&self, row: usize, x: f32, window: &Window) -> Option<usize> {
        let cells = self.shown.get(row)?;
        let cell = f32::from(self.cell());
        let measure = self.measure(window);
        let target = x - self.origin.0;
        let (mut px_at, mut col) = (0.0f32, 0usize);
        for part in cells {
            for ch in part.text.chars() {
                let span = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                let drawn = if part.paint.face == Face::Mono {
                    cell * span as f32
                } else {
                    // `measure` counts zoom-1 cells; `cell` carries the zoom.
                    measure(ch.encode_utf8(&mut [0; 4]), part.paint) * cell
                };
                if px_at + drawn / 2. > target {
                    return Some(col);
                }
                px_at += drawn;
                col += span;
            }
        }
        Some(col + ((target - px_at) / cell).max(0.) as usize)
    }

    fn cell_at(&self, x: Pixels, y: Pixels) -> Option<(usize, usize)> {
        let (x, y) = (f32::from(x), f32::from(y));
        let area = self.hit_area;
        if x < area.left || y < area.top || y > area.bottom {
            return None;
        }
        let (cell, line) = (f32::from(self.cell()), self.line());
        if y >= area.field_top {
            // The field shows the row from the column's start; the chat counts
            // cells from window column 0.
            let col = ((x - area.composer.0) / cell).max(0.) as usize + area.column_start;
            let row =
                area.first_input + ((y - area.composer.1) / self.input_line()).max(0.) as usize;
            let row = row.min(area.last_input);
            return Some((row, col));
        }
        let col = ((x - self.origin.0) / cell).max(0.) as usize;
        // Mid-glide the rows are drawn `shift` px down from their cells.
        let y = y - area.shift;
        let row = (((y - self.origin.1) / line).max(0.) as usize).min(area.above.saturating_sub(1));
        Some((row, col))
    }

    fn on_click(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // A click anywhere else closes the effort panel.
        if self.chat.effort_panel_open() {
            self.chat.close_effort_panel();
            cx.notify();
            return;
        }
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
        let col = if row < self.hit_area.above {
            self.col_at(row, f32::from(event.position.x), window)
                .unwrap_or(col)
        } else {
            col
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
                Some((row, col)) => {
                    let col = if row < self.hit_area.above {
                        self.col_at(row, f32::from(event.position.x), window)
                            .unwrap_or(col)
                    } else {
                        col
                    };
                    self.chat.drag(row, col)
                }
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
        // Anywhere in the text field (its label's band too) places the caret.
        if y < area.field_top || y > area.bottom || x < area.left {
            return None;
        }
        let row = area.first_input + ((y - area.composer.1) / self.input_line()).max(0.) as usize;
        let row = row.min(area.last_input);
        let (start, end) = self.chat.input_span(row)?;
        let text = &self.chat.input_text()[start..end];
        let measure = self.measure(window);
        let cell = f32::from(self.cell());
        // The window's prompt rows have no marker: text starts at the
        // field's text inset.
        let mut left = area.composer.0;
        for (at, ch) in text.char_indices() {
            let width = measure(ch.encode_utf8(&mut [0; 4]), layout::INPUT) * cell;
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
    /// The composer's input row (BodyLarge line).
    input: f32,
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

/// A prompt row as the text field's input: typed text, the selection and
/// the caret (an empty run).
fn composer_row(cells: Row, focused: bool, ui: &Ui) -> gpui::AnyElement {
    use m3_text_field::Span;
    let spans = cells
        .into_iter()
        .filter_map(|run| match run.paint {
            paint if paint == layout::CARET => Some(Span::Caret),
            _ if run.text.is_empty() => None,
            paint if paint == layout::SELECT => Some(Span::Selected(run.text.into())),
            _ => Some(Span::Text(run.text.into())),
        })
        .collect();
    m3_text_field::input_row(spans, focused, ui)
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
        Face::Input => m3_text_field::INPUT.size,
        _ => SANS_SIZE,
    }
}

fn text_run(text: &str, paint: Paint) -> TextRun {
    // Cards draw prompt and code backgrounds; the cells keep only marks
    // that are not panel fill (selection, inline highlights). Typed text is
    // the text field's (`m3_text_field::input_row`).
    let background = paint.bg.filter(|bg| *bg != NIGHT.surface).map(color);
    let fg = color(paint.fg);
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
        color: fg,
        background_color: background,
        underline: paint.underline.then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(fg),
            wavy: false,
        }),
        strikethrough: paint.strike.then(|| StrikethroughStyle {
            thickness: px(1.),
            color: Some(fg),
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
        .bg(color(PANE))
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
    let track = shroom_ui::motion::mix(PANE, M3.surface_container_high, f64::from(open));
    let thumb = shroom_ui::motion::mix(M3.outline_variant, M3.outline, f64::from(open));
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
fn popup(
    view: Popup,
    hints: Vec<String>,
    m: Metrics,
    ui: &Ui,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let open = view.open.clamp(0.0, 1.0) as f32;
    let grow = (0.96 + 0.04 * view.open) as f32;
    let row_h = m3_list::HEIGHT[0] * m.zoom;
    let width = (f32::from(m.cell) * view.width as f32 + 32. * m.zoom).max(360. * m.zoom) * grow;
    let (window, shift) = (view.window, view.shift);
    let selected = view.selected;
    let root = cx.entity();
    // The rows: one-line list items (selected = the highlighted one), the
    // name as headline and the hint as trailing supporting text.
    let rows: Vec<_> = view
        .labels
        .into_iter()
        .zip(view.items)
        .enumerate()
        .skip(1)
        .map(|(n, (label, item))| {
            let (name, hint) = label.unwrap_or_default();
            let row = m3_list::ListItem::new(format!("dialog-row-{n}"), name)
                .container(M3.surface_container_high)
                .selected(selected == Some(n));
            let row = if hint.is_empty() {
                row
            } else {
                row.trailing(hint)
            };
            let row = match item {
                Some(index) => {
                    let root = root.clone();
                    row.on_click(move |_, cx| {
                        root.update(cx, |root, cx| {
                            let outcome = root.chat.pick(index);
                            root.apply(outcome, cx);
                            cx.notify();
                        })
                    })
                }
                None => row.enabled(false),
            };
            div()
                .h(px(row_h))
                .child(row.render(ui, cx))
                .into_any_element()
        })
        .collect();
    // The search field: a filled text field (the same component as the
    // composer), its text and caret drawn by `input_row`.
    let field = view.field.map(|(text, placeholder)| {
        let populated = !text.is_empty();
        let mut spans = Vec::new();
        if populated {
            spans.push(m3_text_field::Span::Text(text.into()));
        }
        spans.push(m3_text_field::Span::Caret);
        m3_text_field::TextField::new("dialog-field", "Search")
            .placeholder(placeholder)
            .leading(m3_state::icon_glyph(
                m3::icon::SEARCH,
                m3_text_field::ICON,
                false,
                m3_text_field::colors(&M3).icon,
                m.zoom,
            ))
            .rows(vec![m3_text_field::input_row(spans, true, ui)])
            .focused(true)
            .populated(populated)
            .render(ui)
            .mx(px(m3::space::S300 * m.zoom))
            .mb(px(m3::space::S100 * m.zoom))
    });
    let header = view.header.map(|header| {
        div()
            .px(px((8. + m3_list::PADDING.1) * m.zoom))
            .mb(px(m3::space::S50 * m.zoom))
            .flex()
            .justify_end()
            .one_line()
            .type_style(m3_list::TRAILING, m.zoom)
            .text_color(color(M3.on_surface_variant))
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
                .px(px(8. * m.zoom))
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
            .bg(color(M3.outline_variant))
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
                    window.paint_quad(gpui::fill(bounds, hsla(0., 0., 0., m3::SCRIM * open)));
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
                .bg(color(M3.surface_container_high))
                .rounded(px(shape::EXTRA_LARGE * m.zoom))
                .shadow(m3_state::shadows(3., m.zoom, M3.shadow))
                .overflow_hidden()
                .py(px(12. * m.zoom))
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(24. * m.zoom))
                        .pt(px(8. * m.zoom))
                        .pb(px(12. * m.zoom))
                        .type_style(m3::font::TITLE_LARGE, m.zoom)
                        .text_color(color(M3.on_surface))
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
                            .mt(px(8. * m.zoom))
                            .mb(px(-12. * m.zoom))
                            .px(px(24. * m.zoom))
                            .h(px(36. * m.zoom))
                            .bg(color(M3.surface_container))
                            .flex()
                            .items_center()
                            .gap(px(16. * m.zoom))
                            .type_style(m3::font::LABEL_MEDIUM, m.zoom)
                            .text_color(color(M3.on_surface_variant))
                            .children(hints.into_iter().map(SharedString::from)),
                    )
                }),
        )
}

/// The left sidebar: product mark, new-session button, the sessions with
/// their state, and the working folder at the bottom.
fn sidebar(
    chrome: &Chrome,
    m: Metrics,
    width: f32,
    ui: &Ui,
    cx: &mut Context<Root>,
) -> gpui::AnyElement {
    let z = m.zoom;
    let mark = img(gpui::ImageSource::Image(std::sync::Arc::new(
        gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            include_bytes!("../../../nix/icons/shroom-64.png").to_vec(),
        ),
    )))
    .size(px(26. * z))
    .rounded(px(shape::SMALL * z));
    // The drawer's primary action: an extended FAB, lowered on its surface.
    let new = div().px(px(12. * z)).pt(px(4. * z)).pb(px(8. * z)).child(
        Fab::new("new-session", m3::icon::EDIT_SQUARE, "New session")
            .extended("New session")
            .lowered()
            .keyshortcuts("Control+N")
            .on_click(press_hint("ctrl+n new", cx))
            .render(ui, cx),
    );
    // The sessions: navigation drawer items (the state as the icon, the
    // age as the badge).
    let sessions: Vec<_> = chrome
        .sessions
        .iter()
        .map(|session| {
            let id = session.id;
            let icon = match session.state.as_str() {
                "answering" => m3::icon::BOLT,
                "stopped" => m3::icon::STOP,
                "" => m3::icon::CHAT_BUBBLE,
                _ => m3::icon::SMART_TOY,
            };
            // `4 turns · 2h ago`: the age is the badge.
            let age = session
                .detail
                .rsplit(" · ")
                .next()
                .unwrap_or_default()
                .trim_end_matches(" ago")
                .to_string();
            let aria = if session.state.is_empty() {
                format!("{}, {}", session.title, session.detail)
            } else {
                format!("{}, {}, {}", session.title, session.state, session.detail)
            };
            let root = cx.entity();
            DrawerItem::new(format!("session-{id}"), session.title.clone())
                .selected(session.current)
                .icon(icon)
                .badge(age)
                .aria(aria)
                .on_click(move |_, cx| {
                    root.update(cx, |root, cx| {
                        root.chat.open_session(id);
                        cx.notify();
                    })
                })
                .render(ui, cx)
        })
        .collect();
    div()
        .id("sidebar")
        .role(gpui::Role::Navigation)
        .aria_label("Sessions")
        .flex_none()
        .w(px(width))
        .h_full()
        .overflow_hidden()
        .bg(color(SHELL))
        .flex()
        .flex_col()
        .font_family(SANS)
        .text_size(px(13. * z))
        .child(
            div()
                .h(px(HEADER * z))
                .px(px(20. * z))
                .pt(px(4. * z))
                .flex()
                .items_center()
                .gap(px(10. * z))
                .child(mark)
                .child(
                    div()
                        .type_style(m3::font::TITLE_MEDIUM, z)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(M3.on_surface))
                        .child("shroom"),
                ),
        )
        .child(new)
        .child(m3_nav::headline("Sessions", ui))
        .child(
            div()
                .id("session-list")
                .role(gpui::Role::TabList)
                .aria_label("Sessions")
                .flex_1()
                .overflow_y_scroll()
                .pb(px(12. * z))
                .flex()
                .flex_col()
                .gap(px(m3_nav::ITEM_SPACE * z))
                .children(sessions),
        )
        .into_any_element()
}

/// The title bar: sidebar toggle, the session's name and folder, new
/// session. Quiet, no rule under it; the model and context live by the
/// composer.
/// The small top app bar (`AppBarSmallTokens`, `AppBarTokens`): the
/// navigation icon, the title over its subtitle, one action.
fn header(chrome: &Chrome, ui: &Ui, cx: &mut Context<Root>) -> gpui::AnyElement {
    let z = ui.zoom;
    let title = m3::font::TITLE_LARGE;
    let subtitle = m3::font::LABEL_MEDIUM;
    div()
        .id("header")
        .role(gpui::Role::Banner)
        .flex_none()
        .h(px(HEADER * z))
        .pl(px(m3::space::S50 * z))
        .pr(px(m3::space::S50 * z))
        .flex()
        .items_center()
        .bg(color(M3.surface))
        .font_family(SANS)
        .child(
            IconButton::new(
                "toggle-sidebar",
                IconKind::Standard,
                m3::icon::MENU,
                "Toggle sidebar",
            )
            // The navigation icon takes the bar's leading icon colour.
            .content(M3.on_surface)
            .keyshortcuts("Control+B")
            .on_click(press_hint("ctrl+b sidebar", cx))
            .render(ui, cx),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .px(px(4. * z))
                .flex()
                .flex_col()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(title.size * z))
                        .line_height(px(title.line * z))
                        .font_weight(title.weight)
                        .text_color(color(M3.on_surface))
                        .child(SharedString::from(chrome.title.clone())),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(subtitle.size * z))
                        .line_height(px(subtitle.line * z))
                        .font_weight(subtitle.weight)
                        .text_color(color(M3.on_surface_variant))
                        .child(SharedString::from(chrome.folder.clone())),
                ),
        )
        .child(
            IconButton::new(
                "new-session-top",
                IconKind::Standard,
                m3::icon::EDIT_SQUARE,
                "New session",
            )
            .keyshortcuts("Control+N")
            .on_click(press_hint("ctrl+n new", cx))
            .render(ui, cx),
        )
        .into_any_element()
}

/// Runs a key hint (as its key would) when clicked.
fn press_hint(
    hint: &'static str,
    cx: &mut Context<Root>,
) -> impl Fn(&mut Window, &mut App) + 'static {
    let root = cx.entity();
    move |_, cx| {
        root.update(cx, |root, cx| {
            let outcome = root.chat.press(hint);
            root.apply(outcome, cx);
            cx.notify();
        });
    }
}

/// The context meter under the composer: a small ring and the share used.
fn context_meter(share: f64, m: Metrics) -> impl IntoElement {
    let z = m.zoom;
    let share = share.clamp(0.0, 1.0) as f32;
    let tone = if share > 0.8 { M3.tertiary } else { M3.primary };
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
        .type_style(m3::font::LABEL_SMALL, z)
        .text_color(color(M3.on_surface_variant))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    let centre = bounds.center();
                    let r = bounds.size.width.min(bounds.size.height) / 2. - px(1.);
                    ring(window, centre, r, 1., color(M3.surface_container_highest));
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
fn welcome(info: &Welcome, m: Metrics, ui: &Ui, cx: &mut Context<Root>) -> impl IntoElement {
    let z = m.zoom;
    let mark = img(gpui::ImageSource::Image(std::sync::Arc::new(
        gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            include_bytes!("../../../nix/icons/shroom-128.png").to_vec(),
        ),
    )))
    .size(px(72. * z))
    .rounded(px(shape::EXTRA_LARGE * z));
    let fact = |label: &str, value: String, mono: bool| {
        div()
            .flex()
            .gap(px(10. * z))
            .child(
                div()
                    .w(px(84. * z))
                    .text_color(color(M3.on_surface_variant))
                    .child(SharedString::from(label.to_string())),
            )
            .child(
                div()
                    .when(mono, |value| value.font_family(FONT).text_size(px(13. * z)))
                    .text_color(color(M3.on_surface))
                    .child(SharedString::from(value)),
            )
    };
    // The suggestions: a segmented list (Compose's recommended list of
    // choices, `SegmentedListItem` on `surfaceContainer`).
    let count = info.starters.len();
    let root = cx.entity();
    let starters: Vec<_> = info
        .starters
        .iter()
        .enumerate()
        .map(|(n, text)| {
            let root = root.clone();
            m3_list::ListItem::new(format!("starter-{n}"), text.clone())
                .segmented(n, count)
                .container(M3.surface_container)
                .on_click(move |_, cx| {
                    root.update(cx, |root, cx| {
                        root.chat.use_starter(n);
                        cx.notify();
                    })
                })
                .render(ui, cx)
        })
        .collect();
    let starters = m3_list::segmented_list("Suggestions", starters, ui);
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
                                        .type_style(m3::font::HEADLINE_MEDIUM, z)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(color(M3.on_surface))
                                        .child("What should we work on?"),
                                )
                                .child(
                                    div()
                                        .text_color(color(M3.on_surface_variant))
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
                                    .px(px(16. * z))
                                    .py(px(12. * z))
                                    .rounded(px(shape::LARGE * z))
                                    .bg(color(M3.error_container))
                                    .text_color(color(M3.on_error_container))
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
                        .child(
                            div()
                                .type_style(m3::font::TITLE_SMALL, z)
                                .text_color(color(M3.primary))
                                .child("Try"),
                        )
                        .child(starters),
                )
                .child(
                    div()
                        .type_style(m3::font::BODY_SMALL, z)
                        .text_color(color(M3.on_surface_variant))
                        .child("F1 lists every key. Ctrl+P runs any command."),
                ),
        )
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Typing goes to the composer unless a component holds focus.
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        let z = self.zoom;
        let m = Metrics {
            cell: self.cell(),
            line: self.line(),
            input: self.input_line(),
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
        // The composer's text field stands its icons beside the reading
        // column; where the window is too narrow for that, the text gives
        // up the cells the icons need (last frame's column).
        let (_, field_w) = composer_span(&self.column, m, main_w);
        let text_w = field_w - 2. * m3_text_field::text_inset(true) * z;
        let lost = f32::from(m.cell) * self.column.len() as f32 - text_w;
        self.chat
            .set_input_room((lost / f32::from(m.cell)).ceil().max(0.) as usize);
        // Header, the gap, the text field's padding and the status row are
        // chrome; the rest is the chat's grid. Input rows are BodyLarge
        // lines, taller than grid lines: last frame's count makes room.
        let input_rows =
            self.hit_area.last_input + 1 - self.hit_area.first_input.min(self.hit_area.last_input);
        let chrome_h =
            (HEADER + COMPOSER_GAP + STATUS + m3_text_field::TEXT_TOP + m3_text_field::VERTICAL)
                * z
                + input_rows as f32 * (m.input - m.line);
        // The grid counts a rule and a status row the window doesn't draw.
        let rows = ((f32::from(viewport.height) - chrome_h - m.pad / 2.) / m.line + 2.).max(4.);
        let frame = self.chat.frame(columns as usize, rows as usize);
        self.rendered = std::time::Instant::now();
        if self.chat.animating() {
            window.request_animation_frame();
        }
        self.shown = frame.rows[..frame.viewport.min(frame.rows.len())].to_vec();
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
        // Clicks map to grid cells from here. The body stacks from the
        // window's bottom edge (left-over pixels go above the transcript):
        // the status row, the text field (its first text row under the
        // floated label), the gap, the chips, the dock, the transcript.
        let above = frame.rule;
        let first_input = frame.composer.start;
        let chip_rows = usize::from(frame.chips.is_some());
        let column = frame.column.clone();
        self.column = column.clone();
        let (field_left, _) = composer_span(&column, m, main_w);
        let field_h = m3_text_field::height(frame.composer.len()) * z;
        let field_top = f32::from(viewport.height) - STATUS * z - field_h;
        let transcript_top = field_top
            - COMPOSER_GAP * z
            - (frame.viewport + frame.dock.len() + chip_rows) as f32 * m.line;
        self.origin = (sidebar_w + m.pad, transcript_top);
        self.hit_area = HitArea {
            left: sidebar_w,
            top: HEADER * z,
            bottom: f32::from(viewport.height) - STATUS * z,
            above,
            field_top,
            shift: frame.shift * m.line,
            composer: (
                sidebar_w + m.pad + field_left + m3_text_field::text_inset(true) * z,
                field_top + m3_text_field::TEXT_TOP * z,
            ),
            first_input,
            last_input: frame.composer.end.saturating_sub(1),
            column_start: column.start,
        };
        let sink: m3_state::Sink = {
            let root = cx.entity().downgrade();
            std::rc::Rc::new(move |id, event, _, cx| {
                let _ = root.update(cx, |root, cx| {
                    root.chat.control(id, event);
                    cx.notify();
                });
            })
        };
        // The composer's field is focused while typing lands in it: the
        // window's focus is on it and no popup holds the keys.
        let field_spec = shroom_ui::motion::FieldSpec {
            focused: self.focus.is_focused(window) && frame.popup.is_none(),
            populated: self.chat.input_populated(),
        };
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
        );
        let head = chrome.as_ref().map(|chrome| header(chrome, &ui, cx));
        // The dialog covers the whole window (sidebar and title bar too).
        let mut frame = frame;
        let dialog = frame.popup.take().map(|view| {
            let hints = chrome
                .as_ref()
                .map(|chrome| chrome.hints.clone())
                .unwrap_or_default();
            popup(view, hints, m, &ui, cx).into_any_element()
        });
        let body = chat_body(
            frame,
            chrome.clone(),
            m,
            columns as usize,
            main_w,
            field_spec,
            &ui,
            cx,
        );
        let side = (sidebar_w > 1.)
            .then(|| {
                chrome
                    .as_ref()
                    .map(|chrome| sidebar(chrome, m, sidebar_w, &ui, cx))
            })
            .flatten();
        ui.finish();
        let main = div()
            .id("main")
            .role(gpui::Role::Main)
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .bg(color(PANE))
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
            .text_color(color(M3.on_surface))
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

/// The composer's text field across the body (from its left padding, px):
/// its text lines up with the reading column, the icons' boxes stand
/// outside it, and it stays inside the body's padding.
fn composer_span(column: &Range<usize>, m: Metrics, main_w: f32) -> (f32, f32) {
    let inset = m3_text_field::text_inset(true) * m.zoom;
    let room = (main_w - 2. * m.pad).max(0.);
    let left = (f32::from(m.cell) * column.start as f32 - inset).max(0.);
    let right = (f32::from(m.cell) * column.end as f32 + inset).min(room);
    let min = (m3_text_field::MIN_WIDTH * m.zoom).min(room);
    (left.min(room - min), (right - left).max(min))
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
    main_w: f32,
    field_spec: shroom_ui::motion::FieldSpec,
    ui: &Ui,
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
        content: _,
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
            Panel::Prompt => (BUBBLE, shape::LARGE_INCREASED),
            Panel::Code => (CODE_CARD, shape::MEDIUM),
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
                code.border_1()
                    .border_color(color(M3.surface_container_high))
            })
            // `fade` already carries the transcript's fade (`content`).
            .opacity(card.fade)
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
            area.child(welcome(&info, m, ui, cx))
        });
    // Below the transcript: dock rows, then (skipping the old rule) the
    // chips, the composer's text field, and the status row.
    let first = viewport;
    let take = |range: Range<usize>, below: &mut Vec<ZonedRow>| {
        range
            .filter_map(|at| below.get(at - first).cloned())
            .map(|(cells, zones)| row(cells, &zones, m))
            .collect::<Vec<_>>()
    };
    let dock_rows = take(dock.clone(), &mut below);
    // Prompt rows: their cells from the column's start, inside the field
    // (the grid centred them with leading spaces).
    let input_rows = composer
        .clone()
        .filter_map(|at| below.get(at - first).cloned())
        .map(|(cells, _)| {
            composer_row(
                layout::slice(&cells, column.start..usize::MAX),
                field_spec.focused,
                ui,
            )
        })
        .collect::<Vec<_>>();
    // Attached images wait as a row of chips above the field.
    let chip_rows = chips
        .map(|at| take(at..at + 1, &mut below))
        .unwrap_or_default();

    // The composer's text field: its text on the reading column, the icons
    // beside it.
    let (field_left, field_w) = composer_span(&column, m, main_w);
    let generating = chrome.as_ref().is_some_and(|chrome| chrome.generating);
    let can_send = chrome.as_ref().is_some_and(|chrome| chrome.can_send);
    let root = cx.entity();
    let attach = IconButton::new(
        "attach",
        IconKind::Standard,
        m3::icon::ATTACH_FILE,
        "Attach files",
    )
    .on_click(move |_, cx| {
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
    .render(ui, cx);
    // The model chip: a filter chip, selected while its menu is open (or,
    // for a model without effort levels, opening the model list).
    let chip = chrome.as_ref().map(|chrome| {
        let open = chrome.effort.as_ref().is_some_and(|panel| panel.expanded);
        let has_levels = chrome.effort.is_some();
        let root = cx.entity();
        m3_chip::FilterChip::new(MODEL_CHIP, chip_label(chrome))
            .menu(open)
            .leading(m3::icon::SMART_TOY)
            .trailing(m3::icon::ARROW_DROP_DOWN)
            .aria(format!("Model {}", chip_label(chrome)))
            .on_click(move |_, cx| {
                root.update(cx, |root, cx| {
                    if has_levels {
                        root.chat.toggle_effort_panel();
                    } else {
                        let outcome = root.chat.press("ctrl+l model");
                        root.apply(outcome, cx);
                    }
                    cx.notify();
                });
            })
            .render(ui, cx)
    });
    let panel = chrome.as_ref().and_then(|chrome| {
        let panel = chrome.effort.clone()?;
        // Unmounted once it has closed (Compose: when the transition settles);
        // its groups then start afresh, not yet hovered.
        if !panel.expanded && panel.open <= 0.001 {
            for group in 0..2 {
                ui.forget(m3_menu::group_id(EFFORT_MENU, group));
            }
            return None;
        }
        Some(effort_menu(chrome.model.clone(), panel, m, ui, cx))
    });
    let field = m3_text_field::TextField::new(COMPOSER, "Message")
        .multiline()
        .placeholder(shroom_ui::chat::PLACEHOLDER)
        .leading(attach)
        .trailing(send_button(generating, can_send, ui, cx))
        .rows(input_rows)
        .focused(field_spec.focused)
        .populated(field_spec.populated)
        .render(ui)
        .ml(px(field_left))
        .w(px(field_w))
        .mt(px(COMPOSER_GAP * z));
    // Under the composer: the model chip and a notice on the left, the
    // context ring on the right; the effort menu opens above the chip.
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
                .text_color(color(if good { M3.primary } else { M3.tertiary }))
                .when(good, |line| line.child("✓"))
                .child(SharedString::from(text))
        });
    let context = chrome
        .as_ref()
        .and_then(|chrome| chrome.context)
        .map(|share| context_meter(share, m));
    let status = div()
        .relative()
        .flex_none()
        .ml(px(field_left))
        .w(px(field_w))
        .h(px(STATUS * z))
        .flex()
        .items_center()
        .justify_between()
        .type_style(m3::font::BODY_SMALL, z)
        .text_color(color(M3.on_surface_variant))
        .child(
            div()
                .min_w(px(0.))
                .flex()
                .items_center()
                .gap(px(m3::space::S100 * z))
                .children(chip)
                .children(notice),
        )
        // The ring stands under the end of the text, inside the field's
        // trailing icon box.
        .child(
            div()
                .flex_none()
                .pr(px(m3_text_field::text_inset(true) * z))
                .children(context),
        )
        .children(panel);
    let mut bars_el = Vec::new();
    for mut bar in bars
        .into_iter()
        .filter(|bar| bar.id == layout::TRANSCRIPT_BAR)
    {
        // At the window's edge, not beside the text (where it read as a rule).
        bar.col = window_cols.saturating_sub(1);
        bars_el.push(scrollbar(bar, m, cx));
    }
    // Pixels left over by the whole-row grid go above the transcript, so
    // the composer always sits on the window's bottom edge.
    div()
        .flex_1()
        .min_h(px(0.))
        .relative()
        .px(px(m.pad))
        .pt(px(m.pad / 2.))
        .flex()
        .flex_col()
        .justify_end()
        .child(transcript)
        .child(div().flex().flex_col().children(dock_rows))
        .child(div().flex().flex_col().children(chip_rows))
        .child(field)
        .child(status)
        .children(bars_el)
        .into_any_element()
}

/// Send (or stop, while answering): a filled icon button, disabled when
/// there is nothing to send.
fn send_button(
    generating: bool,
    can_send: bool,
    ui: &Ui,
    cx: &mut Context<Root>,
) -> gpui::AnyElement {
    let (label, glyph) = if generating {
        ("Stop", m3::icon::STOP)
    } else {
        ("Send", m3::icon::ARROW_UPWARD)
    };
    let root = cx.entity();
    IconButton::new("send", IconKind::Filled, glyph, label)
        .size(Size::Small)
        .enabled(generating || can_send)
        .on_click(move |_, cx| {
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
        .render(ui, cx)
}

/// The chip's text: the model and its effort level.
fn chip_label(chrome: &Chrome) -> String {
    match chrome
        .effort
        .as_ref()
        .and_then(|panel| panel.levels.get(panel.selected))
    {
        Some(level) => format!("{} · {level}", chrome.model),
        None => chrome.model.clone(),
    }
}

/// The model chip's control key.
const MODEL_CHIP: &str = "model-chip";
/// The composer's text field's control key.
const COMPOSER: &str = "composer";
const EFFORT_MENU: &str = "effort-menu";

/// The model and effort menu above the chip (Compose's expressive
/// `DropdownMenuPopup`): a group with the model, which opens the model
/// list, and a group of the effort levels to pick from. It scales from 0.8
/// and fades in; the scale is drawn as a zoom, pivoting where Compose's
/// transform origin is: the bottom (it opens above its anchor), at the
/// centre of where it overlaps the chip.
fn effort_menu(
    model: String,
    panel: shroom_ui::chat::EffortPanel,
    m: Metrics,
    ui: &Ui,
    cx: &mut Context<Root>,
) -> gpui::AnyElement {
    // The fast spatial spring overshoots 1 by about 2 % before it settles.
    // Compose draws that as a sub-pixel scale of the finished layer; here the
    // menu is laid out again at each size, so the overshoot came out as the
    // box and its re-rasterised text stepping a pixel out and back over the
    // last frames. It is drawn up to 1 and held there.
    let s = panel.scale.clamp(0., 1.);
    let z = m.zoom * s;
    // Its width, from the text at full size: the scale then shrinks one
    // fixed shape (Compose's graphicsLayer) instead of laying it out anew.
    let width = {
        let text = |line: &str, ty| m3_state::text_width(line, ty, cx);
        let model_text =
            text(&model, m3_menu::LABEL).max(text("Change model", m3_menu::SUPPORTING));
        let (_, start, end) = m3_menu::GROUP_LABEL;
        let widest = panel
            .levels
            .iter()
            .map(|level| m3_menu::item_width(text(level, m3_menu::LABEL), true, false))
            .fold(m3_menu::item_width(model_text, true, true), f32::max)
            .max(start + text("Thinking effort", m3_menu::LABEL) + end);
        // Kerning and rounding slack.
        m3_menu::width(widest + 2.)
    };
    let root = cx.entity();
    let groups = [
        m3_menu::Group::new(EFFORT_MENU, 0, 2, ui),
        m3_menu::Group::new(EFFORT_MENU, 1, 2, ui),
    ];
    // Every height in the menu is measured off one ruler, top to bottom, so
    // the menu grows a device pixel at a time as it scales (see `Ruler`).
    let mut ruler = m3_state::Ruler::new(z, ui.device);
    let model_top = m3_menu::Group::open(&mut ruler);
    let pick_model = {
        let root = root.clone();
        m3_menu::MenuItem::new("effort-menu-model", model.clone())
            .supporting("Change model")
            .leading(m3::icon::SMART_TOY)
            .trailing(m3::icon::CHEVRON_RIGHT)
            .aria(format!("Model {model}. Change model"))
            .edges(&groups[0], true, true)
            .ruled(&mut ruler)
            .on_click(move |_, cx| {
                root.update(cx, |root, cx| {
                    root.chat.close_effort_panel();
                    let outcome = root.chat.press("ctrl+l model");
                    root.apply(outcome, cx);
                    cx.notify();
                })
            })
            .render(ui, z, cx)
    };
    let [model_group, level_group] = groups;
    let model_group = model_group.render(model_top, vec![pick_model], &mut ruler, ui, z);
    let gap = m3_menu::gap(&mut ruler);
    let levels_top = m3_menu::Group::open(&mut ruler);
    let count = panel.levels.len();
    let mut levels = vec![m3_menu::group_label("Thinking effort", &mut ruler, ui, z)];
    for (n, level) in panel.levels.iter().enumerate() {
        let root = root.clone();
        let item = m3_menu::MenuItem::new(format!("effort-menu-level-{n}"), level.clone())
            .selected(n == panel.selected)
            .selected_leading(m3::icon::CHECK)
            .place(n, count)
            .edges(&level_group, false, n + 1 == count)
            .ruled(&mut ruler)
            .on_click(move |_, cx| {
                root.update(cx, |root, cx| {
                    root.chat.set_effort_at(n);
                    cx.notify();
                })
            })
            .render(ui, z, cx);
        levels.push(item);
    }
    let groups = vec![
        model_group,
        gap,
        level_group.render(levels_top, levels, &mut ruler, ui, z),
    ];
    // The pivot: the centre of the menu's overlap with the chip (both
    // left-aligned), so the left edge moves in by (1 - s) of half of it.
    let chip = ui
        .places()
        .borrow()
        .get(&m3_state::control_id(MODEL_CHIP))
        .map_or(0., |bounds| f32::from(bounds.size.width));
    let inward = (1. - s) * chip.min(width * m.zoom) / 2.;
    m3_menu::popup(EFFORT_MENU, "Model and effort", groups, panel.open)
        .absolute()
        .w(px(width * z))
        // Left-aligned with the chip, which starts the status row.
        .left(px(inward))
        // Above its anchor, the chip's 48 target (the status row).
        .bottom(px(STATUS * m.zoom))
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(m3_state::measure(
            ui.places(),
            m3_state::control_id(EFFORT_MENU),
        ))
        .into_any_element()
}

/// The bundled faces: Shroom Sans and the Material Symbols icon fonts.
fn load_fonts(window: &Window) {
    let added = window.text_system().add_fonts(vec![
        std::borrow::Cow::Borrowed(include_bytes!("../fonts/ShroomSans-Regular.ttf").as_slice()),
        std::borrow::Cow::Borrowed(include_bytes!("../fonts/ShroomSans-SemiBold.ttf").as_slice()),
        std::borrow::Cow::Borrowed(include_bytes!("../fonts/ShroomSans-Italic.ttf").as_slice()),
        std::borrow::Cow::Borrowed(include_bytes!("../fonts/ShroomSans-Medium.ttf").as_slice()),
        std::borrow::Cow::Borrowed(
            include_bytes!("../fonts/MaterialSymbols-Outlined.ttf").as_slice(),
        ),
        std::borrow::Cow::Borrowed(
            include_bytes!("../fonts/MaterialSymbols-Filled.ttf").as_slice(),
        ),
    ]);
    // gpui keeps going without a font it could not read; the families
    // still resolve (to a fallback), so a failure here is only logged.
    if let Err(err) = added {
        eprintln!("shroom: bundled fonts: {err}");
    }
}

fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("shroom {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let gallery = std::env::args().any(|arg| arg == "--gallery");
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(900.), px(640.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // Matches the launcher entry (StartupWMClass) so docks and
            // window lists show it as shroom.
            app_id: Some("shroom".into()),
            ..Default::default()
        };
        let opened = if gallery {
            // Every component in its states (see `m3_gallery`).
            cx.open_window(options, |window, cx| {
                cx.new(|cx| m3_gallery::Gallery::new(window, cx))
            })
            .map(drop)
        } else {
            cx.open_window(options, |window, cx| cx.new(|cx| Root::new(window, cx)))
                .map(drop)
        };
        if let Err(err) = opened {
            eprintln!("shroom: cannot open a window: {err}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}
