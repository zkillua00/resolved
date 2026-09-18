//! Read-only document backend of CodeEditor. Coordinates and selections belong
//! to the complete document; only viewport text is copied/shaped. In particular
//! a long natural line is never turned into a single GPUI ShapedLine.
use std::{
    io::Read,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::{
    App, AppContext as _, Bounds, ClipboardItem, ContentMask, Context, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, HighlightStyle, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _,
    Pixels, Point, Render, ScrollWheelEvent, ShapedLine, SharedString, Styled as _, Subscription,
    Task, TextRun, UTF16Selection, Window, canvas, div, fill, point, prelude::FluentBuilder as _,
    px, size,
};
use gpui_component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Copy, Input, InputEvent, InputState, Search, SelectAll},
};

use super::{
    CodeLanguage, EditorText,
    document::{Document, Row},
};
use crate::{
    core::{FormatterSettings, ResponseBody},
    theme::ApiThemeExt as _,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct ViewOptions {
    pub tab_size: usize,
    pub soft_wrap: bool,
    pub line_numbers: bool,
    pub indent_guides: bool,
    pub active_line: bool,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    anchor: usize,
    head: usize,
}

impl Selection {
    fn range(self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Viewport {
    row: usize,
    column: usize,
    rows: usize,
    columns: usize,
}

struct FrameRow {
    row: Row,
    styles: Vec<(Range<usize>, HighlightStyle)>,
    first_visual_row: bool,
}

struct PaintedRow {
    row: Row,
    layout: ShapedLine,
    origin: Point<Pixels>,
}

#[derive(Clone, Copy)]
enum Drag {
    Selection,
    Vertical { grab: f32 },
    Horizontal { grab: f32 },
}

/// Cancellation outlives the GPUI task so background workers also stop when an
/// editor/tab disappears. Closing/replacing a response releases all its owners.
struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
}

impl Drop for Cancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub(super) struct ReadOnlyEditor {
    focus: FocusHandle,
    options: ViewOptions,
    language: CodeLanguage,
    source: Option<ResponseBody>,
    prepared: Option<ResponseBody>,
    pretty: bool,
    formatter: FormatterSettings,
    document: Option<Document>,
    error: Option<String>,
    build: Task<()>,
    build_cancel: Cancellation,
    generation: u64,
    wrap_columns: Option<usize>,
    selection: Selection,
    preferred_column: Option<usize>,
    top: usize,
    left: usize,
    wheel_x: f64,
    wheel_y: f64,
    bounds: Bounds<Pixels>,
    gutter: Pixels,
    cell_width: Pixels,
    line_height: Pixels,
    viewport: Option<Viewport>,
    frame: Vec<FrameRow>,
    painted: Vec<PaintedRow>,
    frame_task: Task<()>,
    frame_generation: u64,
    frame_theme: Option<Arc<gpui_component::highlighter::HighlightTheme>>,
    vertical_thumb: Option<Bounds<Pixels>>,
    horizontal_thumb: Option<Bounds<Pixels>>,
    drag: Option<Drag>,
    find: Entity<InputState>,
    find_open: bool,
    find_case_sensitive: bool,
    find_status: SharedString,
    find_task: Task<()>,
    find_cancel: Cancellation,
    find_generation: u64,
    copy_task: Task<()>,
    copy_on_ready: bool,
    _find_subscription: Subscription,
}

impl ReadOnlyEditor {
    pub fn new(
        options: ViewOptions,
        language: CodeLanguage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let find = cx.new(|cx| InputState::new(window, cx).placeholder("Find in response"));
        let subscription = cx.subscribe(&find, |this, _, event, cx| match event {
            InputEvent::Change => this.find_next(false, true, cx),
            InputEvent::PressEnter { secondary } => this.find_next(*secondary, false, cx),
            _ => {}
        });
        Self {
            focus: cx.focus_handle(),
            options,
            language,
            source: None,
            prepared: None,
            pretty: false,
            formatter: FormatterSettings::default(),
            document: None,
            error: None,
            build: Task::ready(()),
            build_cancel: Cancellation::new(),
            generation: 0,
            wrap_columns: None,
            selection: Selection::default(),
            preferred_column: None,
            top: 0,
            left: 0,
            wheel_x: 0.,
            wheel_y: 0.,
            bounds: Bounds::default(),
            gutter: px(0.),
            cell_width: px(8.),
            line_height: px(20.),
            viewport: None,
            frame: Vec::new(),
            painted: Vec::new(),
            frame_task: Task::ready(()),
            frame_generation: 0,
            frame_theme: None,
            vertical_thumb: None,
            horizontal_thumb: None,
            drag: None,
            find,
            find_open: false,
            find_case_sensitive: false,
            find_status: "".into(),
            find_task: Task::ready(()),
            find_cancel: Cancellation::new(),
            find_generation: 0,
            copy_task: Task::ready(()),
            copy_on_ready: false,
            _find_subscription: subscription,
        }
    }

    pub fn set_source(
        &mut self,
        source: ResponseBody,
        pretty: bool,
        formatter: FormatterSettings,
        cx: &mut Context<Self>,
    ) {
        if self.source.as_ref().is_some_and(|current| {
            current.as_ptr() == source.as_ptr() && current.len() == source.len()
        }) && self.pretty == pretty
            && self.formatter == formatter
        {
            return;
        }
        self.source = Some(source);
        self.pretty = pretty;
        self.formatter = formatter;
        self.prepared = None;
        self.document = None;
        self.copy_on_ready = false;
        self.selection = Selection::default();
        self.top = 0;
        self.left = 0;
        self.rebuild(cx);
    }

    pub fn set_language(&mut self, language: CodeLanguage, cx: &mut Context<Self>) {
        if self.language != language {
            self.language = language;
            self.viewport = None;
            cx.notify();
        }
    }

    pub fn set_options(&mut self, options: ViewOptions, cx: &mut Context<Self>) {
        if self.options == options {
            return;
        }
        let reindex = self.options.tab_size != options.tab_size
            || self.options.soft_wrap != options.soft_wrap;
        self.options = options;
        if reindex {
            self.wrap_columns = options.soft_wrap.then(|| self.visible_columns());
            self.rebuild(cx);
        }
        cx.notify();
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        self.build_cancel = Cancellation::new();
        let cancel = self.build_cancel.0.clone();
        let prepared = self.prepared.clone();
        let pretty = self.pretty;
        let formatter = self.formatter.clone();
        let tab_size = self.options.tab_size;
        let wrap = self.wrap_columns;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.error = None;
        self.viewport = None;
        self.frame.clear();
        self.painted.clear();
        self.find_cancel = Cancellation::new();
        self.find_task = Task::ready(());
        self.find_generation = self.find_generation.wrapping_add(1);
        let task = cx.background_spawn(async move {
            let body = match prepared {
                Some(body) => body,
                None if pretty => super::pretty::pretty_body(&source, &formatter, &cancel)?,
                None => source,
            };
            let document = Document::new(body.clone(), tab_size, wrap, &cancel)?;
            Ok::<_, String>((body, document))
        });
        self.build = cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                match result {
                    Ok((body, document)) => {
                        this.prepared = Some(body);
                        this.selection.anchor = this.selection.anchor.min(document.len());
                        this.selection.head = this.selection.head.min(document.len());
                        this.document = Some(document);
                        // A resized/wrapped document may have exactly the same
                        // viewport coordinates as the previous index. Invalidate
                        // both ready and in-flight frames from that index.
                        this.viewport = None;
                        this.frame_generation = this.frame_generation.wrapping_add(1);
                        this.frame.clear();
                        this.painted.clear();
                        this.reveal_cursor();
                        if this.find_open {
                            this.find_next(false, true, cx);
                        }
                        if std::mem::take(&mut this.copy_on_ready) {
                            this.copy_all(cx);
                        }
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    pub fn value(&self) -> EditorText {
        match &self.document {
            Some(document) => EditorText::Mapped {
                range: 0..document.len(),
                document: document.clone(),
            },
            None => "".into(),
        }
    }

    pub fn selection_snapshot(&self) -> (Range<usize>, EditorText, EditorText) {
        let value = self.value();
        let Some(document) = &self.document else {
            return (0..0, value, "".into());
        };
        let range = self.selection.range();
        (
            document.utf16_offset(range.start)..document.utf16_offset(range.end),
            value.clone(),
            value.slice(range),
        )
    }

    pub fn copy_all(&mut self, cx: &mut Context<Self>) {
        if let Some(document) = &self.document {
            self.copy_range(0..document.len(), cx);
        } else {
            self.copy_on_ready = true;
        }
    }

    fn copy_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let Some(document) = self.document.clone() else {
            return;
        };
        // Clipboard APIs require an owned string. Only the user's explicit copy
        // materializes it, off the UI thread; no copy is made for selection.
        let task = cx.background_spawn(async move { document.text(range).to_owned() });
        self.copy_task = cx.spawn(async move |this, cx| {
            let text = task.await;
            let _ = this.update(cx, |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            });
        });
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        if let Some(document) = &self.document {
            self.selection = Selection {
                anchor: 0,
                head: document.len(),
            };
            cx.notify();
        }
    }

    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = true;
        self.find.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    fn find_next(&mut self, backwards: bool, from_selection: bool, cx: &mut Context<Self>) {
        self.find_cancel = Cancellation::new();
        self.find_generation = self.find_generation.wrapping_add(1);
        let generation = self.find_generation;
        let Some(document) = self.document.clone() else {
            return;
        };
        let query = self.find.read(cx).value().to_string();
        if query.is_empty() {
            self.find_status = "".into();
            cx.notify();
            return;
        }
        let start = if from_selection || backwards {
            self.selection.range().start
        } else {
            self.selection.range().end
        };
        let case_sensitive = self.find_case_sensitive;
        let cancel = self.find_cancel.0.clone();
        self.find_status = "Searching…".into();
        let task = cx.background_spawn(async move {
            find_in_document(&document, &query, start, backwards, case_sensitive, &cancel)
        });
        self.find_task = cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.find_generation != generation {
                    return;
                }
                if let Some(range) = result {
                    this.selection = Selection {
                        anchor: range.start,
                        head: range.end,
                    };
                    this.find_status = "Match".into();
                    this.reveal_cursor();
                } else {
                    this.find_status = "No matches".into();
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    fn visible_rows(&self) -> usize {
        ((self.bounds.size.height - px(12.)) / self.line_height)
            .floor()
            .max(1.) as usize
    }

    fn visible_columns(&self) -> usize {
        ((self.bounds.size.width - self.gutter - px(20.)) / self.cell_width)
            .floor()
            .max(1.) as usize
    }

    fn scroll_limits(&self) -> (usize, usize) {
        self.document.as_ref().map_or((0, 0), |document| {
            (
                document.display_rows().saturating_sub(self.visible_rows()),
                if self.options.soft_wrap {
                    0
                } else {
                    document
                        .max_columns()
                        .saturating_sub(self.visible_columns())
                },
            )
        })
    }

    fn reveal_cursor(&mut self) {
        let Some(document) = &self.document else {
            return;
        };
        let point = document.position(self.selection.head);
        let rows = self.visible_rows();
        let columns = self.visible_columns();
        if point.row < self.top {
            self.top = point.row;
        }
        if point.row >= self.top.saturating_add(rows) {
            self.top = point.row.saturating_sub(rows - 1);
        }
        if point.column < self.left {
            self.left = point.column;
        }
        if point.column >= self.left.saturating_add(columns) {
            self.left = point.column.saturating_sub(columns - 1);
        }
        let (max_top, max_left) = self.scroll_limits();
        self.top = self.top.min(max_top);
        self.left = self.left.min(max_left);
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let primary = if cfg!(target_os = "macos") {
            modifiers.platform
        } else {
            modifiers.control
        };
        if key == "escape" && self.find_open {
            self.find_open = false;
            self.focus.focus(window);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if !self.focus.is_focused(window) {
            return;
        }
        if primary {
            match key {
                "c" => {
                    self.copy_range(self.selection.range(), cx);
                    cx.stop_propagation();
                    return;
                }
                "a" => {
                    self.select_all(cx);
                    cx.stop_propagation();
                    return;
                }
                "f" => {
                    self.open_find(window, cx);
                    cx.stop_propagation();
                    return;
                }
                "g" => {
                    self.find_next(modifiers.shift, false, cx);
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }
        let Some(document) = &self.document else {
            return;
        };
        let old = self.selection.head;
        let point = document.position(old);
        let word = if cfg!(target_os = "macos") {
            modifiers.alt
        } else {
            modifiers.control
        };
        let document_motion =
            primary && (cfg!(target_os = "macos") || matches!(key, "home" | "end"));
        let target = match key {
            "left" if document_motion => document.line_range(old).start,
            "right" if document_motion => {
                let range = document.line_range(old);
                let end = range.end;
                if document.text(range).ends_with('\n') {
                    document.previous_offset(end)
                } else {
                    end
                }
            }
            "left" if word => document.word_left(old),
            "right" if word => document.word_right(old),
            "left" if !modifiers.shift && !self.selection.range().is_empty() => {
                self.selection.range().start
            }
            "right" if !modifiers.shift && !self.selection.range().is_empty() => {
                self.selection.range().end
            }
            "left" => document.previous_offset(old),
            "right" => document.next_offset(old),
            "up" if document_motion => 0,
            "down" if document_motion => document.len(),
            "home" if document_motion => 0,
            "end" if document_motion => document.len(),
            "home" => document.offset_at(point.row, 0),
            "end" => document.offset_at(point.row, usize::MAX),
            "up" | "down" | "pageup" | "pagedown" => {
                let distance = if key.starts_with("page") {
                    self.visible_rows()
                } else {
                    1
                };
                let row = if key.ends_with("up") {
                    point.row.saturating_sub(distance)
                } else {
                    point.row.saturating_add(distance)
                };
                let column = *self.preferred_column.get_or_insert(point.column);
                document.offset_at(row, column)
            }
            _ => return,
        };
        if !matches!(key, "up" | "down" | "pageup" | "pagedown") {
            self.preferred_column = None;
        }
        if !modifiers.shift {
            self.selection.anchor = target;
        }
        self.selection.head = target;
        self.reveal_cursor();
        cx.stop_propagation();
        cx.notify();
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(self.line_height);
        let (dx, dy) = if event.modifiers.shift && delta.x == px(0.) {
            (delta.y, px(0.))
        } else {
            (delta.x, delta.y)
        };
        self.wheel_y -= f64::from(dy / self.line_height);
        self.wheel_x -= f64::from(dx / self.cell_width);
        let (max_top, max_left) = self.scroll_limits();
        self.top = add_scroll(self.top, &mut self.wheel_y, max_top);
        self.left = add_scroll(self.left, &mut self.wheel_x, max_left);
        cx.stop_propagation();
        cx.notify();
    }

    fn hit_offset(&self, position: Point<Pixels>) -> Option<usize> {
        let row = ((position.y - self.bounds.top()) / self.line_height)
            .floor()
            .max(0.) as usize;
        let painted = self
            .painted
            .get(row.min(self.painted.len().saturating_sub(1)))?;
        let ix = painted
            .layout
            .closest_index_for_x(position.x - painted.origin.x);
        Some(painted.row.source_offset(ix))
    }

    pub fn context_click(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window);
        if let Some(offset) = self.hit_offset(event.position)
            && !self.selection.range().contains(&offset)
        {
            self.selection = Selection {
                anchor: offset,
                head: offset,
            };
        }
        cx.notify();
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window);
        if let Some(thumb) = self
            .vertical_thumb
            .filter(|bounds| bounds.contains(&event.position))
        {
            self.drag = Some(Drag::Vertical {
                grab: f32::from(event.position.y - thumb.top()),
            });
        } else if let Some(thumb) = self
            .horizontal_thumb
            .filter(|bounds| bounds.contains(&event.position))
        {
            self.drag = Some(Drag::Horizontal {
                grab: f32::from(event.position.x - thumb.left()),
            });
        } else if event.position.x >= self.bounds.right() - px(12.) {
            self.drag = Some(Drag::Vertical { grab: 0. });
            self.drag_to(event.position);
        } else if event.position.y >= self.bounds.bottom() - px(12.) {
            self.drag = Some(Drag::Horizontal { grab: 0. });
            self.drag_to(event.position);
        } else if let Some(offset) = self.hit_offset(event.position) {
            let document = self.document.as_ref().unwrap();
            if event.click_count >= 3 {
                let range = document.line_range(offset);
                self.selection = Selection {
                    anchor: range.start,
                    head: range.end,
                };
            } else if event.click_count == 2 {
                let range = document.word_range(offset);
                self.selection = Selection {
                    anchor: range.start,
                    head: range.end,
                };
            } else {
                if !event.modifiers.shift {
                    self.selection.anchor = offset;
                }
                self.selection.head = offset;
            }
            self.drag = Some(Drag::Selection);
            self.preferred_column = None;
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn drag_to(&mut self, position: Point<Pixels>) {
        let (max_top, max_left) = self.scroll_limits();
        match self.drag {
            Some(Drag::Selection) => {
                if position.y < self.bounds.top() {
                    self.top = self.top.saturating_sub(1);
                }
                if position.y >= self.bounds.bottom() {
                    self.top = self.top.saturating_add(1).min(max_top);
                }
                if position.x < self.bounds.left() + self.gutter {
                    self.left = self.left.saturating_sub(1);
                }
                if position.x >= self.bounds.right() {
                    self.left = self.left.saturating_add(1).min(max_left);
                }
                if let Some(offset) = self.hit_offset(position) {
                    self.selection.head = offset;
                }
            }
            Some(Drag::Vertical { grab }) => {
                let thumb = self
                    .vertical_thumb
                    .map_or(px(0.), |bounds| bounds.size.height);
                let travel = (self.bounds.size.height - px(12.) - thumb).max(px(1.));
                let ratio = ((position.y - self.bounds.top() - px(grab)) / travel).clamp(0., 1.);
                self.top = (f64::from(ratio) * max_top as f64).round() as usize;
            }
            Some(Drag::Horizontal { grab }) => {
                let thumb = self
                    .horizontal_thumb
                    .map_or(px(0.), |bounds| bounds.size.width);
                let travel = (self.bounds.size.width - self.gutter - px(12.) - thumb).max(px(1.));
                let ratio = ((position.x - self.bounds.left() - self.gutter - px(grab)) / travel)
                    .clamp(0., 1.);
                self.left = (f64::from(ratio) * max_left as f64).round() as usize;
            }
            None => {}
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !event.dragging() {
            self.drag = None;
            return;
        }
        if self.drag.is_some() {
            self.drag_to(event.position);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn prepare_frame(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.bounds = bounds;
        let style = cx.api_theme().classes.editor.clone();
        self.line_height = style.font_size * (20. / 13.);
        let font = gpui::font(style.font_family);
        let font_size = style.font_size;
        self.cell_width = window
            .text_system()
            .shape_line(
                "m".into(),
                font_size,
                &[TextRun {
                    len: 1,
                    font,
                    color: cx.theme().foreground,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
            .width
            .max(px(1.));
        let digits = self.document.as_ref().map_or(4, |doc| {
            doc.line_number_at(doc.len()).to_string().len().max(4)
        });
        self.gutter = if self.options.line_numbers {
            self.cell_width * (digits + 3) as f32
        } else {
            px(8.)
        };
        let columns = self.visible_columns();
        let wrap = self.options.soft_wrap.then_some(columns);
        if wrap != self.wrap_columns {
            self.wrap_columns = wrap;
            self.rebuild(cx);
        }
        let Some(document) = self.document.clone() else {
            return;
        };
        let (max_top, max_left) = self.scroll_limits();
        self.top = self.top.min(max_top);
        self.left = self.left.min(max_left);
        let viewport = Viewport {
            row: self.top,
            column: self.left,
            rows: self.visible_rows() + 1,
            columns: columns + 2,
        };
        let theme = cx.theme().highlight_theme.clone();
        if self.viewport == Some(viewport)
            && self
                .frame_theme
                .as_ref()
                .is_some_and(|previous| Arc::ptr_eq(previous, &theme))
        {
            return;
        }
        self.frame_theme = Some(theme.clone());
        self.viewport = Some(viewport);
        self.frame.clear();
        self.painted.clear();
        self.frame_generation = self.frame_generation.wrapping_add(1);
        let generation = self.frame_generation;
        let document_generation = self.generation;
        let language = self.language.clone();
        let task = cx.background_spawn(async move {
            (viewport.row
                ..viewport
                    .row
                    .saturating_add(viewport.rows)
                    .min(document.display_rows()))
                .map(|index| {
                    let row = document.row(index, viewport.column, viewport.columns);
                    let row_start = document.offset_at(index, 0);
                    let first_visual_row = document.line_range(row_start).start == row_start;
                    let styles = if language.as_str() == "json" {
                        json_styles(
                            &row.text,
                            document.json_in_string(row.range.start),
                            document.json_escaped(row.range.start),
                            &theme,
                        )
                    } else if language.as_str() != "text" && !row.text.is_empty() {
                        // Only this display fragment enters the syntax parser,
                        // never the complete source or a giant natural line.
                        let mut highlighter =
                            gpui_component::highlighter::SyntaxHighlighter::new(language.as_str());
                        let text = gpui_component::input::Rope::from(row.text.as_str());
                        highlighter.update(None, &text);
                        highlighter.styles(&(0..row.text.len()), &theme)
                    } else {
                        Vec::new()
                    };
                    FrameRow {
                        row,
                        styles,
                        first_visual_row,
                    }
                })
                .collect::<Vec<_>>()
        });
        self.frame_task = cx.spawn(async move |this, cx| {
            let rows = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.frame_generation == generation && this.generation == document_generation {
                    this.frame = rows;
                    cx.notify();
                }
            });
        });
    }

    fn paint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bounds = self.bounds;
        window.handle_input(
            &self.focus,
            ElementInputHandler::new(bounds, cx.entity()),
            cx,
        );
        let editor_style = cx.api_theme().classes.editor.clone();
        let font_size = editor_style.font_size;
        let font = gpui::font(editor_style.font_family);
        let theme = cx.theme();
        let foreground = theme
            .highlight_theme
            .style
            .editor_foreground
            .unwrap_or(theme.foreground);
        let gutter_color = theme
            .highlight_theme
            .style
            .editor_gutter_background
            .unwrap_or(cx.api_surface_lowest());
        let line_number = theme
            .highlight_theme
            .style
            .editor_line_number
            .unwrap_or(theme.muted_foreground);
        let active_line = theme.highlight_theme.style.editor_active_line;
        let selection_color = theme.selection;
        let caret_color = theme.caret;
        let thumb_color = theme.scrollbar_thumb;
        let run = |len, color| TextRun {
            len,
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        self.painted.clear();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(error) = self.error.as_ref() {
                let text = format!("Could not open response: {error}");
                let line = window.text_system().shape_line(
                    text.clone().into(),
                    font_size,
                    &[run(text.len(), foreground)],
                    None,
                );
                let _ = line.paint(
                    bounds.origin + point(px(8.), px(4.)),
                    self.line_height,
                    window,
                    cx,
                );
                return;
            }
            if self.document.is_none() {
                let label = "Preparing response document…";
                let line = window.text_system().shape_line(
                    label.into(),
                    font_size,
                    &[run(label.len(), foreground)],
                    None,
                );
                let _ = line.paint(
                    bounds.origin + point(px(8.), px(4.)),
                    self.line_height,
                    window,
                    cx,
                );
                return;
            }
            let selection = self.selection.range();
            let document = self.document.as_ref().unwrap();
            let cursor = document.position(self.selection.head);
            let text_bounds = Bounds::new(
                bounds.origin + point(self.gutter, px(0.)),
                size(
                    (bounds.size.width - self.gutter - px(12.)).max(px(0.)),
                    (bounds.size.height - px(12.)).max(px(0.)),
                ),
            );
            window.with_content_mask(
                Some(ContentMask {
                    bounds: text_bounds,
                }),
                |window| {
                    for (index, frame) in self.frame.iter().enumerate() {
                        let row = &frame.row;
                        let y = bounds.top() + self.line_height * index as f32;
                        let origin = point(
                            bounds.left()
                                + self.gutter
                                + self.cell_width
                                    * (row.start_column as f64 - self.left as f64) as f32,
                            y,
                        );
                        if self.options.active_line
                            && cursor.row == self.top + index
                            && let Some(color) = active_line
                        {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(text_bounds.left(), y),
                                    size(text_bounds.size.width, self.line_height),
                                ),
                                color,
                            ));
                        }
                        let mut runs = Vec::new();
                        let mut previous = 0;
                        for (range, style) in &frame.styles {
                            if previous < range.start {
                                runs.push(run(range.start - previous, foreground));
                            }
                            runs.push(run(range.len(), style.color.unwrap_or(foreground)));
                            previous = range.end;
                        }
                        if previous < row.text.len() {
                            runs.push(run(row.text.len() - previous, foreground));
                        }
                        let layout = window.text_system().shape_line(
                            row.text.clone().into(),
                            font_size,
                            &runs,
                            None,
                        );
                        if self.options.indent_guides {
                            let leading = row.text.bytes().take_while(|byte| *byte == b' ').count();
                            let tab = self.options.tab_size.max(1);
                            for column in row.start_column..row.start_column.saturating_add(leading)
                            {
                                if column > 0 && column % tab == 0 {
                                    let x = self.cell_width * (column - row.start_column) as f32;
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            origin + point(x, px(0.)),
                                            size(px(1.), self.line_height),
                                        ),
                                        line_number.opacity(0.2),
                                    ));
                                }
                            }
                        }
                        if selection.start < row.range.end && selection.end > row.range.start {
                            let start = layout.x_for_index(
                                row.rendered_offset(selection.start.max(row.range.start)),
                            );
                            let end = layout
                                .x_for_index(row.rendered_offset(selection.end.min(row.range.end)));
                            window.paint_quad(fill(
                                Bounds::new(
                                    origin + point(start, px(0.)),
                                    size((end - start).max(px(3.)), self.line_height),
                                ),
                                selection_color,
                            ));
                        }
                        let _ = layout.paint(origin, self.line_height, window, cx);
                        if self.focus.is_focused(window)
                            && cursor.row == self.top + index
                            && row.range.start <= self.selection.head
                            && self.selection.head <= row.range.end
                        {
                            let x = layout.x_for_index(row.rendered_offset(self.selection.head));
                            window.paint_quad(fill(
                                Bounds::new(
                                    origin + point(x, px(0.)),
                                    size(px(1.5), self.line_height),
                                ),
                                caret_color,
                            ));
                        }
                        self.painted.push(PaintedRow {
                            row: row.clone(),
                            layout,
                            origin,
                        });
                    }
                },
            );
            if self.options.line_numbers {
                let gutter_bounds = Bounds::new(
                    bounds.origin,
                    size(self.gutter - px(4.), bounds.size.height),
                );
                window.paint_quad(fill(gutter_bounds, gutter_color));
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: gutter_bounds,
                    }),
                    |window| {
                        for (index, frame) in self.frame.iter().enumerate() {
                            if !frame.first_visual_row {
                                continue;
                            }
                            let text = frame.row.line_number.to_string();
                            let number = window.text_system().shape_line(
                                text.clone().into(),
                                font_size,
                                &[run(text.len(), line_number)],
                                None,
                            );
                            let _ = number.paint(
                                point(
                                    bounds.left() + self.gutter
                                        - number.width
                                        - self.cell_width * 2.,
                                    bounds.top() + self.line_height * index as f32,
                                ),
                                self.line_height,
                                window,
                                cx,
                            );
                        }
                    },
                );
            }
            let (max_top, max_left) = self.scroll_limits();
            self.vertical_thumb = scrollbar_thumb(
                bounds,
                self.gutter,
                self.top,
                max_top,
                self.visible_rows(),
                true,
            );
            self.horizontal_thumb = scrollbar_thumb(
                bounds,
                self.gutter,
                self.left,
                max_left,
                self.visible_columns(),
                false,
            );
            for thumb in [self.vertical_thumb, self.horizontal_thumb]
                .into_iter()
                .flatten()
            {
                window.paint_quad(fill(thumb, thumb_color));
            }
        });
    }
}

impl Focusable for ReadOnlyEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for ReadOnlyEditor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let document = self.document.as_ref()?;
        let start = document.byte_offset(range.start);
        let end = document.byte_offset(range.end).max(start);
        *adjusted = Some(document.utf16_offset(start)..document.utf16_offset(end));
        Some(document.text(start..end).to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let document = self.document.as_ref()?;
        let range = self.selection.range();
        Some(UTF16Selection {
            range: document.utf16_offset(range.start)..document.utf16_offset(range.end),
            reversed: self.selection.head < self.selection.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let document = self.document.as_ref()?;
        let start = document.byte_offset(range.start);
        let end = document.byte_offset(range.end);
        let row = self
            .painted
            .iter()
            .find(|row| row.row.range.start <= start && start <= row.row.range.end)?;
        let left = row.layout.x_for_index(row.row.rendered_offset(start));
        let right = row
            .layout
            .x_for_index(row.row.rendered_offset(end.min(row.row.range.end)));
        Some(Bounds::new(
            row.origin + point(left, px(0.)),
            size((right - left).max(px(1.)), self.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.hit_offset(point)?;
        Some(self.document.as_ref()?.utf16_offset(offset))
    }
}

impl Render for ReadOnlyEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let paint_entity = entity.clone();
        div()
            .id("read-only-document")
            .debug_selector(|| "code-editor-mapped-document".to_owned())
            .size_full()
            .v_flex()
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::key_down))
            .on_action(
                cx.listener(|this, _: &Copy, _, cx| this.copy_range(this.selection.range(), cx)),
            )
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &Search, window, cx| this.open_find(window, cx)))
            .when(self.find_open, |this| {
                this.child(
                    h_flex()
                        .w_full()
                        .gap_1()
                        .p_1()
                        .child(Input::new(&self.find).small().w(px(260.)))
                        .child(
                            Button::new("mapped-find-case")
                                .small()
                                .ghost()
                                .label("Aa")
                                .selected(self.find_case_sensitive)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.find_case_sensitive = !this.find_case_sensitive;
                                    this.find_next(false, true, cx);
                                })),
                        )
                        .child(
                            Button::new("mapped-find-previous")
                                .small()
                                .ghost()
                                .icon(IconName::ChevronUp)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.find_next(true, false, cx)),
                                ),
                        )
                        .child(
                            Button::new("mapped-find-next")
                                .small()
                                .ghost()
                                .icon(IconName::ChevronDown)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.find_next(false, false, cx)),
                                ),
                        )
                        .child(div().text_xs().child(self.find_status.clone()))
                        .child(
                            Button::new("mapped-find-close")
                                .small()
                                .ghost()
                                .icon(IconName::Close)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.find_open = false;
                                    this.focus.focus(window);
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                div()
                    .id("mapped-document-viewport")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .cursor_text()
                    .on_scroll_wheel(cx.listener(Self::scroll))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                    .on_mouse_move(cx.listener(Self::mouse_move))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.drag = None),
                    )
                    .child(
                        canvas(
                            move |bounds, window, cx| {
                                entity.update(cx, |editor, cx| {
                                    editor.prepare_frame(bounds, window, cx)
                                })
                            },
                            move |_, _, window, cx| {
                                paint_entity.update(cx, |editor, cx| editor.paint(window, cx))
                            },
                        )
                        .size_full(),
                    ),
            )
    }
}

fn add_scroll(current: usize, pending: &mut f64, maximum: usize) -> usize {
    let whole = pending.trunc();
    *pending -= whole;
    (current as f64 + whole).clamp(0., maximum as f64) as usize
}

fn scrollbar_thumb(
    bounds: Bounds<Pixels>,
    gutter: Pixels,
    offset: usize,
    maximum: usize,
    visible: usize,
    vertical: bool,
) -> Option<Bounds<Pixels>> {
    if maximum == 0 {
        return None;
    }
    let track = if vertical {
        bounds.size.height - px(12.)
    } else {
        bounds.size.width - gutter - px(12.)
    };
    let thumb = (track * (visible as f64 / maximum.saturating_add(visible) as f64) as f32)
        .max(px(24.))
        .min(track);
    let start = (track - thumb) * (offset as f64 / maximum as f64) as f32;
    Some(if vertical {
        Bounds::new(
            point(bounds.right() - px(10.), bounds.top() + start),
            size(px(8.), thumb),
        )
    } else {
        Bounds::new(
            point(bounds.left() + gutter + start, bounds.bottom() - px(10.)),
            size(thumb, px(8.)),
        )
    })
}

// Searches all source bytes, retaining only the next/previous match rather than
// a potentially enormous array of matches. Aho-Corasick carries state between
// reads, so long/repetitive patterns don't rescan an overlap at every byte.
fn find_in_document(
    document: &Document,
    query: &str,
    start: usize,
    backwards: bool,
    case_sensitive: bool,
    cancel: &AtomicBool,
) -> Option<Range<usize>> {
    if query.is_empty() || document.is_empty() {
        return None;
    }
    let text = document.all_text().as_bytes();
    let start = start.min(text.len());
    let matcher = aho_corasick::AhoCorasick::builder()
        .ascii_case_insensitive(!case_sensitive)
        .prefilter(false)
        .kind(Some(aho_corasick::AhoCorasickKind::ContiguousNFA))
        .build([query])
        .ok()?;
    let scan = |range: Range<usize>, reverse: bool| {
        let mut result = None;
        let reader = SearchReader {
            bytes: &text[range.clone()],
            cancel,
        };
        for found in matcher.stream_find_iter(reader) {
            let found = found.ok()?;
            let matched = range.start + found.start()..range.start + found.end();
            if !reverse {
                return Some(matched);
            }
            result = Some(matched);
        }
        result
    };
    if backwards {
        scan(0..start, true).or_else(|| scan(0..text.len(), true))
    } else {
        scan(start..text.len(), false).or_else(|| scan(0..text.len(), false))
    }
}

struct SearchReader<'a> {
    bytes: &'a [u8],
    cancel: &'a AtomicBool,
}

impl Read for SearchReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("Search cancelled"));
        }
        let len = buffer.len().min(self.bytes.len()).min(64 * 1024);
        buffer[..len].copy_from_slice(&self.bytes[..len]);
        self.bytes = &self.bytes[len..];
        Ok(len)
    }
}

fn json_styles(
    text: &str,
    mut in_string: bool,
    mut escaped: bool,
    theme: &gpui_component::highlighter::HighlightTheme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let bytes = text.as_bytes();
    let mut result = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        let name;
        if in_string || bytes[at] == b'"' {
            name = "string";
            if !in_string {
                at += 1;
            }
            in_string = true;
            while at < bytes.len() {
                if escaped {
                    escaped = false;
                    at += text[at..].chars().next().unwrap().len_utf8();
                    continue;
                }
                match bytes[at] {
                    b'\\' => {
                        at += 1;
                        escaped = true;
                    }
                    b'"' => {
                        at += 1;
                        in_string = false;
                        break;
                    }
                    _ => at += 1,
                }
            }
        } else if matches!(bytes[at], b'0'..=b'9' | b'-') {
            name = "number";
            at += 1;
            while at < bytes.len()
                && matches!(bytes[at], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
            {
                at += 1;
            }
        } else if bytes[at].is_ascii_alphabetic() {
            name = "boolean";
            at += 1;
            while at < bytes.len() && bytes[at].is_ascii_alphabetic() {
                at += 1;
            }
        } else {
            at += 1;
            continue;
        }
        if let Some(style) = theme.style(name) {
            result.push((start..at, style));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_editor::{CodeEditor, CodeEditorConfig};
    use gpui::{Modifiers, ScrollDelta, TestAppContext};
    use std::io::Write;

    fn mapped_body(bytes: &[u8]) -> ResponseBody {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(bytes).unwrap();
        // SAFETY: the completed anonymous fixture has a single owner.
        unsafe { ResponseBody::file(file) }.unwrap()
    }

    fn doc(text: &str) -> Document {
        Document::new(
            mapped_body(text.as_bytes()),
            4,
            None,
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn search_spans_pages_wraps_and_preserves_unicode_offsets() {
        let prefix = "a".repeat(65_534);
        let document = doc(&format!("{prefix}NeEdle🙂\nneedle"));
        let cancel = AtomicBool::new(false);
        assert_eq!(
            find_in_document(&document, "needle", 0, false, false, &cancel),
            Some(65_534..65_540)
        );
        assert_eq!(
            find_in_document(&document, "needle", 65_540, false, false, &cancel),
            Some(65_545..65_551)
        );
        assert_eq!(
            find_in_document(&document, "needle", 65_551, false, false, &cancel),
            Some(65_534..65_540)
        );
        assert_eq!(
            find_in_document(&document, "needle", 65_545, true, false, &cancel),
            Some(65_534..65_540)
        );
        assert_eq!(
            find_in_document(&document, "🙂", 0, false, true, &cancel),
            Some(65_540..65_544)
        );
        assert_eq!(
            find_in_document(&document, "NEEDLE", 0, false, true, &cancel),
            None
        );
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            find_in_document(&document, "needle", 0, false, false, &cancel),
            None
        );
    }

    #[test]
    fn repetitive_case_insensitive_search_has_linear_work() {
        let document = doc(&"a".repeat(256 * 1024));
        let needle = format!("{}b", "a".repeat(64 * 1024));
        assert_eq!(
            find_in_document(&document, &needle, 0, false, false, &AtomicBool::new(false)),
            None
        );
    }

    #[test]
    fn search_wrap_includes_occurrence_containing_cursor() {
        let document = doc("abcdef");
        for backwards in [false, true] {
            assert_eq!(
                find_in_document(
                    &document,
                    "bcde",
                    3,
                    backwards,
                    false,
                    &AtomicBool::new(false)
                ),
                Some(1..5),
            );
        }
    }

    #[test]
    fn json_fragment_carries_quote_escape_state() {
        let theme = gpui_component::highlighter::HighlightTheme::default_dark();
        let text = r#""still in string", 123"#;
        let styles = json_styles(text, true, true, &theme);
        assert_eq!(styles[0].0, 0..17);
        assert_eq!(styles.last().unwrap().0, 19..22);
    }

    #[gpui::test]
    fn wide_text_scrolls_and_wraps_without_losing_glyphs(cx: &mut TestAppContext) {
        use unicode_width::UnicodeWidthStr as _;
        let text = format!("{}🙂e\u{301}", "界".repeat(60));
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let view = cx.new(|cx| {
                let mut editor = CodeEditor::new(
                    CodeEditorConfig::default()
                        .read_only(true)
                        .soft_wrap(false)
                        .framed(false),
                    window,
                    cx,
                );
                editor.set_response_body(
                    mapped_body(text.as_bytes()),
                    false,
                    FormatterSettings::default(),
                    window,
                    cx,
                );
                editor
            });
            editor = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        cx.simulate_resize(size(px(600.), px(320.)));
        cx.run_until_parked();
        let editor = editor.unwrap();
        let mapped = cx.read(|cx| editor.read(cx).mapped.clone().unwrap());
        cx.update(|window, cx| editor.read(cx).focus_handle(cx).focus(window));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-down"
        } else {
            "ctrl-end"
        });
        cx.run_until_parked();
        cx.read(|cx| {
            let state = mapped.read(cx);
            assert_eq!(state.document.as_ref().unwrap().max_columns(), text.width());
            assert!(state.scroll_limits().1 > 0);
            assert!(state.left > 0);
            assert!(state.frame[0].row.text.ends_with("🙂e\u{301}"));
        });
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| editor.set_soft_wrap(true, window, cx))
        });
        cx.run_until_parked();
        cx.read(|cx| {
            let state = mapped.read(cx);
            let document = state.document.as_ref().unwrap();
            assert!(document.display_rows() > 1);
            let rows = (0..document.display_rows())
                .map(|row| document.row(row, 0, state.wrap_columns.unwrap()))
                .collect::<Vec<_>>();
            assert!(
                rows.iter()
                    .all(|row| row.text.width() <= state.wrap_columns.unwrap())
            );
            assert_eq!(
                rows.iter().map(|row| row.text.as_str()).collect::<String>(),
                text
            );
        });
    }

    /// Opt-in disk/GUI regression using the original failure's response size.
    /// No 2GiB fixture or String is constructed in the test process.
    #[gpui::test]
    #[ignore = "writes and indexes a complete 2GiB response"]
    fn two_gib_single_line_remains_mapped_in_normal_editor(cx: &mut TestAppContext) {
        let mut file = tempfile::tempfile().unwrap();
        const BODY_BYTES: usize = 2 * 1024 * 1024 * 1024;
        let chunk = [b'a'; 64 * 1024];
        for _ in 0..BODY_BYTES / chunk.len() {
            file.write_all(&chunk).unwrap();
        }
        // SAFETY: single-owner anonymous file, all writes complete.
        let body = unsafe { ResponseBody::file(file) }.unwrap();
        let address = body.as_ptr() as usize;
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let view = cx.new(|cx| {
                let mut editor = CodeEditor::new(
                    CodeEditorConfig::default()
                        .language(CodeLanguage::Plain)
                        .read_only(true)
                        .soft_wrap(false)
                        .framed(false),
                    window,
                    cx,
                );
                editor.set_response_body(body, false, FormatterSettings::default(), window, cx);
                editor
            });
            editor = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        cx.simulate_resize(size(px(800.), px(500.)));
        cx.run_until_parked();
        let editor = editor.unwrap();
        let mapped = cx.read(|cx| editor.read(cx).mapped.clone().unwrap());
        cx.update(|window, cx| editor.read(cx).focus_handle(cx).focus(window));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-down"
        } else {
            "ctrl-end"
        });
        cx.run_until_parked();
        cx.read(|cx| {
            let state = mapped.read(cx);
            let document = state.document.as_ref().unwrap();
            assert_eq!(document.len(), BODY_BYTES);
            assert_eq!(document.all_text().as_ptr() as usize, address);
            assert_eq!(state.selection.head, BODY_BYTES);
            assert!(state.frame[0].row.range.start > BODY_BYTES - 512);
            assert!(state.frame[0].row.text.len() < 256);
            assert_eq!(document.display_rows(), 1);
            assert!(editor.read(cx).input.read(cx).value().is_empty());
        });
        cx.simulate_keystrokes("shift-left shift-left");
        cx.update(|window, cx| {
            let (range, snapshot, selected) =
                editor.update(cx, |editor, cx| editor.selection_snapshot(window, cx));
            assert_eq!(range, BODY_BYTES - 2..BODY_BYTES);
            assert_eq!(snapshot.as_ptr() as usize, address);
            assert_eq!(selected.as_ref(), "aa");
        });
    }

    #[gpui::test]
    fn mapped_code_editor_keeps_navigation_selection_search_and_copy(cx: &mut TestAppContext) {
        let text = format!("{}\nneedle🙂\nlast", "0123456789".repeat(64 * 1024));
        let body = mapped_body(text.as_bytes());
        let source_ptr = body.as_ptr() as usize;
        let len = body.len();
        let menu_opened = std::rc::Rc::new(std::cell::Cell::new(false));
        let observed_menu = menu_opened.clone();
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let view = cx.new(|cx| {
                let mut editor = CodeEditor::new(
                    CodeEditorConfig::default()
                        .language(CodeLanguage::Plain)
                        .read_only(true)
                        .soft_wrap(false)
                        .framed(false)
                        .context_menu_builder(std::rc::Rc::new(move |menu, context, _, _| {
                            assert_eq!(context.document.as_ptr() as usize, source_ptr);
                            assert_eq!(context.range, 1..3);
                            assert_eq!(context.selected_text.as_ref(), "12");
                            observed_menu.set(true);
                            menu
                        })),
                    window,
                    cx,
                );
                editor.set_response_body(body, false, FormatterSettings::default(), window, cx);
                editor
            });
            editor = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        cx.simulate_resize(size(px(640.), px(320.)));
        cx.run_until_parked();
        let editor = editor.unwrap();
        let mapped = cx.read(|cx| editor.read(cx).mapped.clone().unwrap());
        cx.read(|cx| {
            let state = mapped.read(cx);
            assert!(state.document.is_some());
            assert!(state.error.is_none());
            assert_eq!(
                state.value().as_ptr() as usize,
                source_ptr,
                "snapshot must borrow the original file"
            );
            assert_eq!(state.value().len(), len);
            assert!(
                state
                    .frame
                    .iter()
                    .map(|row| row.row.text.len())
                    .sum::<usize>()
                    < 4096
            );
            assert!(
                editor.read(cx).input.read(cx).value().is_empty(),
                "no mapped body in the Rope"
            );
        });
        cx.update(|window, cx| editor.read(cx).focus_handle(cx).focus(window));
        cx.simulate_keystrokes("right shift-right shift-right");
        cx.update(|window, cx| {
            let (range, document, selected) =
                editor.update(cx, |editor, cx| editor.selection_snapshot(window, cx));
            assert_eq!(range, 1..3);
            assert_eq!(selected.as_ref(), "12");
            assert_eq!(document.as_ptr() as usize, source_ptr);
        });
        cx.run_until_parked();
        let context_point = cx.read(|cx| {
            let row = &mapped.read(cx).painted[0];
            row.origin + point(row.layout.x_for_index(2), px(4.))
        });
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: context_point,
            ..Default::default()
        });
        assert!(
            menu_opened.get(),
            "mapped editor must use the normal menu decorator"
        );
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        });
        cx.run_until_parked();
        assert_eq!(
            cx.read(|cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            "12"
        );
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-down"
        } else {
            "ctrl-end"
        });
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| mapped.read(cx).selection.head), len);
        cx.simulate_keystrokes("shift-left shift-left shift-left shift-left");
        cx.update(|window, cx| {
            assert_eq!(
                editor
                    .update(cx, |editor, cx| editor.selection_snapshot(window, cx))
                    .2
                    .as_ref(),
                "last"
            );
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-f"
        } else {
            "ctrl-f"
        });
        cx.simulate_input("needle");
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(
                editor
                    .update(cx, |editor, cx| editor.selection_snapshot(window, cx))
                    .2
                    .as_ref(),
                "needle"
            );
        });
        cx.simulate_keystrokes("escape");
        cx.simulate_input("not editable");
        assert_eq!(cx.read(|cx| editor.read(cx).value(cx).len()), len);
        // Ordinary horizontal scrolling is within the long logical line, not
        // artificial 4KiB rows. A small viewport fragment is all that is shaped.
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-up"
        } else {
            "ctrl-home"
        });
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(160.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(-32000.), px(0.))),
            ..Default::default()
        });
        cx.run_until_parked();
        cx.read(|cx| {
            let state = mapped.read(cx);
            assert!(state.left > 1000);
            assert_eq!(state.top, 0);
            assert!(state.frame[0].row.range.start > 1000);
            assert!(state.frame[0].row.text.len() < 256);
        });
        // Mouse selection uses the shaped fragment's mapping to global offsets.
        cx.simulate_click(point(px(130.), px(10.)), Modifiers::none());
        assert!(cx.read(|cx| mapped.read(cx).selection.head) > 1000);
    }

    #[gpui::test]
    fn mapped_editor_reflow_pretty_replacement_and_owner_lifetime(cx: &mut TestAppContext) {
        let raw = br#"{"a":[1,2],"b":"hello"}"#;
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let view = cx.new(|cx| {
                let mut editor = CodeEditor::new(
                    CodeEditorConfig::default()
                        .read_only(true)
                        .soft_wrap(false)
                        .framed(false),
                    window,
                    cx,
                );
                editor.set_response_body(
                    mapped_body(raw),
                    true,
                    FormatterSettings::default(),
                    window,
                    cx,
                );
                editor
            });
            editor = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        cx.simulate_resize(size(px(400.), px(260.)));
        cx.run_until_parked();
        let editor = editor.unwrap();
        let snapshot = cx.read(|cx| editor.read(cx).value(cx));
        assert!(snapshot.contains('\n'));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&snapshot).unwrap(),
            serde_json::from_slice::<serde_json::Value>(raw).unwrap()
        );
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_response_body(
                    mapped_body(&vec![b'x'; 32 * 1024]),
                    false,
                    FormatterSettings::default(),
                    window,
                    cx,
                );
                editor.set_soft_wrap(true, window, cx);
            })
        });
        cx.run_until_parked();
        let mapped = cx.read(|cx| editor.read(cx).mapped.clone().unwrap());
        let old_rows = cx.read(|cx| mapped.read(cx).document.as_ref().unwrap().display_rows());
        cx.simulate_resize(size(px(250.), px(260.)));
        cx.run_until_parked();
        cx.read(|cx| {
            let state = mapped.read(cx);
            assert!(state.document.as_ref().unwrap().display_rows() > old_rows);
            assert!(state.frame.len() > 1);
            assert_eq!(state.frame[0].row.text.len(), state.wrap_columns.unwrap());
        });
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| editor.set_value("small", window, cx))
        });
        assert!(cx.read(|cx| editor.read(cx).mapped.is_none()));
        assert_eq!(cx.read(|cx| editor.read(cx).value(cx).to_string()), "small");
        assert!(
            snapshot.contains("hello"),
            "snapshot outlives replaced backing document"
        );
    }
}
