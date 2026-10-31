//! The desktop transcript, drawn with zeron's markdown renderer (MIT, zeron
//! crates/ui/src/markdown): a virtual list of turns. Prompts are right-aligned
//! bubbles, answers zeron-rendered markdown, finished work one quiet line
//! that opens on click (Claude desktop), reasoning on a faint bar (DeepSeek).
//! The terminal keeps the shared character grid; this is window-only.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Entity, FontWeight, ListAlignment, ListState, SharedString, Window, div,
    prelude::*, px,
};
use shroom_ui::chat::Desk;
use shroom_ui::{Line, Role};
use zeron_ui::markdown::{self, render::RenderOptions};
use zeron_ui::theme::Theme;

/// One row of the list.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Prompt(String),
    /// Answer markdown; `live` while it streams.
    Answer {
        text: String,
        key: String,
    },
    /// A run of reasoning and tool programs: (first line index, its lines).
    Work {
        key: usize,
        lines: Vec<Line>,
        open: bool,
        live: bool,
    },
    Note(String),
    Spinner(String),
}

/// The list and what it was built from.
pub struct Transcript {
    pub list: ListState,
    items: Vec<Item>,
    doc: Option<u64>,
    trees: HashMap<String, (String, Arc<markdown::BlockTree>)>,
    highlights: HashMap<String, Arc<zeron_syntax::HighlightedDocument>>,
}

impl Transcript {
    pub fn new() -> Self {
        let list = ListState::new(0, ListAlignment::Bottom, px(800.));
        Self {
            list,
            items: Vec::new(),
            doc: None,
            trees: HashMap::new(),
            highlights: HashMap::new(),
        }
    }

    /// Rebuild the rows from the chat's transcript; only changed rows are
    /// re-measured, so streaming and resizing never flash.
    pub fn sync(&mut self, desk: &Desk) {
        let items = items(desk);
        if self.doc != Some(desk.doc) {
            self.doc = Some(desk.doc);
            self.list.reset(items.len());
            self.trees.clear();
        } else {
            let same = self
                .items
                .iter()
                .zip(&items)
                .take_while(|(a, b)| a == b)
                .count();
            if same != self.items.len() || same != items.len() {
                self.list.splice(same..self.items.len(), items.len() - same);
            }
        }
        // The newest prompt sits at the top with room for its answer below.
        let last_prompt = items
            .iter()
            .rposition(|item| matches!(item, Item::Prompt(_)));
        self.list
            .set_tail_reservation(last_prompt.map(|at| (at, px(16.))));
        self.items = items;
    }

    /// Row `ix` as an element.
    pub fn row(
        &mut self,
        ix: usize,
        theme: &Theme,
        window: &mut Window,
        on_fold: &dyn Fn(usize) -> Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)>,
    ) -> AnyElement {
        let Some(item) = self.items.get(ix).cloned() else {
            return div().into_any_element();
        };
        let body: AnyElement = match item {
            Item::Prompt(text) => div()
                .flex()
                .justify_end()
                .child(
                    div()
                        .max_w(gpui::relative(0.8))
                        .px(px(14.))
                        .py(px(8.))
                        .rounded(px(16.))
                        .bg(theme.surface_raised.opacity(0.35))
                        .text_color(theme.text)
                        .child(self.markdown(&format!("p{ix}"), &text, theme, window)),
                )
                .into_any_element(),
            Item::Answer { text, key } => self.markdown(&key, &text, theme, window),
            Item::Work {
                key,
                lines,
                open,
                live,
            } => work(key, &lines, open, live, theme, on_fold),
            Item::Note(text) => div()
                .text_color(theme.danger)
                .child(SharedString::from(text))
                .into_any_element(),
            Item::Spinner(text) => div()
                .text_size(px(13.))
                .text_color(theme.text_muted)
                .child(SharedString::from(text))
                .into_any_element(),
        };
        div().pt(px(18.)).child(body).into_any_element()
    }

    /// `source` parsed once per change and drawn by zeron's renderer.
    fn markdown(
        &mut self,
        key: &str,
        source: &str,
        theme: &Theme,
        window: &mut Window,
    ) -> AnyElement {
        let tree = match self.trees.get(key) {
            Some((text, tree)) if text == source => tree.clone(),
            _ => {
                let tree = Arc::new(markdown::parse_full(source));
                self.trees
                    .insert(key.to_string(), (source.to_string(), tree.clone()));
                tree
            }
        };
        let mut highlights = Vec::new();
        for (ix, top) in tree.blocks.iter().enumerate() {
            let document = match &top.block {
                markdown::Block::CodeBlock { language, code } => {
                    let id = format!("{key}:{ix}:{}", code.len());
                    let cached = self.highlights.get(&id).cloned();
                    cached.or_else(|| {
                        let document = zeron_syntax::highlight(zeron_syntax::HighlightRequest {
                            source: code,
                            path: None,
                            fence_tag: language.as_deref(),
                        })
                        .ok()
                        .map(Arc::new)?;
                        self.highlights.insert(id, document.clone());
                        Some(document)
                    })
                }
                _ => None,
            };
            highlights.push(document);
        }
        let opts = RenderOptions::settled(SharedString::from(key.to_string()));
        markdown::render::render_tree(&tree, &opts, theme, window, &|ix| {
            highlights.get(ix).cloned().flatten()
        })
    }
}

/// The transcript as rows: prompts, answers, work runs, notes.
fn items(desk: &Desk) -> Vec<Item> {
    let mut out = Vec::new();
    push_lines(&mut out, &desk.lines, 0, desk, false);
    if !desk.live.is_empty() {
        push_lines(
            &mut out,
            &desk.live,
            desk.lines.len() + 1_000_000,
            desk,
            true,
        );
    }
    if let Some(spinner) = &desk.spinner {
        out.push(Item::Spinner(spinner.clone()));
    }
    out
}

fn push_lines(out: &mut Vec<Item>, lines: &[Line], base: usize, desk: &Desk, live: bool) {
    let mut at = 0;
    while at < lines.len() {
        let role = lines[at].role;
        let same = |line: &Line| match role {
            Role::Thinking | Role::Program | Role::Call | Role::Agent => matches!(
                line.role,
                Role::Thinking | Role::Program | Role::Call | Role::Agent
            ),
            other => line.role == other,
        };
        let end = lines[at..]
            .iter()
            .position(|line| !same(line))
            .map_or(lines.len(), |offset| at + offset);
        let group = &lines[at..end];
        let text = || {
            group
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        };
        match role {
            Role::User => out.push(Item::Prompt(text())),
            Role::Assistant => out.push(Item::Answer {
                text: text(),
                key: format!("a{}", base + at),
            }),
            Role::Thinking | Role::Program | Role::Call | Role::Agent => {
                let key = base + at;
                let running = live && end == lines.len();
                out.push(Item::Work {
                    key,
                    lines: group.to_vec(),
                    open: desk.details || desk.open.contains(&key) || running,
                    live: running,
                })
            }
            Role::Fail => out.push(Item::Note(text())),
            _ => out.push(Item::Answer {
                text: text(),
                key: format!("o{}", base + at),
            }),
        }
        at = end;
    }
}

/// A work run: one quiet line (Claude desktop), opened: reasoning on a faint
/// bar (DeepSeek) and each step under its program.
fn work(
    key: usize,
    lines: &[Line],
    open: bool,
    live: bool,
    theme: &Theme,
    on_fold: &dyn Fn(usize) -> Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)>,
) -> AnyElement {
    let titles: Vec<&str> = lines
        .iter()
        .filter(|line| line.role == Role::Program)
        .map(|line| line.text.split('\t').next().unwrap_or_default())
        .filter(|title| !title.trim().is_empty())
        .collect();
    let calls = lines.iter().filter(|line| line.role == Role::Call).count();
    let thought = lines.iter().any(|line| line.role == Role::Thinking);
    let mut label = if titles.is_empty() {
        if live {
            "Thinking…".to_string()
        } else {
            "Thought".to_string()
        }
    } else {
        titles.join(", ")
    };
    let mut meta = Vec::new();
    if calls > 0 {
        meta.push(format!("{calls} call{}", if calls == 1 { "" } else { "s" }));
    }
    if thought && !titles.is_empty() {
        meta.push("thought".into());
    }
    if !meta.is_empty() {
        label = format!("{label} · {}", meta.join(" · "));
    }
    let header = div()
        .id(("work", key))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(13.5))
        .text_color(theme.text_muted)
        .cursor_pointer()
        .hover(|style| style.text_color(theme.text))
        .on_click(on_fold(key))
        .child(SharedString::from(label))
        .child(div().text_size(px(11.)).child(if open { "⌄" } else { "›" }));
    let mut column = div().flex().flex_col().gap(px(6.)).child(header);
    if open {
        let mut body = div()
            .ml(px(2.))
            .pl(px(12.))
            .border_l_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .gap(px(2.))
            .text_size(px(13.5))
            .line_height(px(21.));
        for line in lines {
            body = body.child(match line.role {
                Role::Thinking => div()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(line.text.clone())),
                Role::Program => {
                    let (title, summary) = line.text.split_once('\t').unwrap_or((&line.text, ""));
                    div()
                        .pt(px(4.))
                        .flex()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(SharedString::from(title.to_string())),
                        )
                        .child(
                            div()
                                .text_color(theme.text_faint)
                                .child(SharedString::from(summary.to_string())),
                        )
                }
                Role::Call => {
                    let mut fields = line.text.splitn(4, '\t');
                    let mark = fields.next().unwrap_or_default();
                    let verb = fields.next().unwrap_or_default();
                    let target = fields.next().unwrap_or_default();
                    let info = fields.next().unwrap_or_default();
                    let tone = match mark {
                        "✗" => theme.danger,
                        "…" => theme.warning,
                        _ => theme.success,
                    };
                    div()
                        .flex()
                        .gap(px(8.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_family(theme.font_mono.clone())
                        .text_size(px(12.5))
                        .child(
                            div()
                                .text_color(tone)
                                .child(SharedString::from(mark.to_string())),
                        )
                        .child(
                            div()
                                .text_color(theme.text_dim)
                                .child(SharedString::from(verb.to_string())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_color(theme.text_muted)
                                .child(SharedString::from(target.to_string())),
                        )
                        .child(
                            div()
                                .text_color(theme.text_faint)
                                .child(SharedString::from(info.to_string())),
                        )
                }
                _ => div()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(line.text.clone())),
            });
        }
        column = column.child(body);
    }
    column.into_any_element()
}

/// Opens or folds a work run, by its key.
pub type OnFold =
    std::rc::Rc<dyn Fn(usize) -> Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)>>;

/// The list element: rows rendered on demand from the transcript entity.
pub fn element(transcript: Entity<Transcript>, on_fold: OnFold, cx: &App) -> AnyElement {
    let state = transcript.read(cx).list.clone();
    gpui::list(state, move |ix, window, cx| {
        let theme = Theme::of(cx).clone();
        let on_fold = on_fold.clone();
        transcript.update(cx, |transcript, _| {
            transcript.row(ix, &theme, window, &*on_fold)
        })
    })
    .size_full()
    .into_any_element()
}
