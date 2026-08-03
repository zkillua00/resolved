use std::{rc::Rc, time::Duration};

use gpui::{
    App, AppContext as _, Context, Corner, DismissEvent, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement as _, Pixels, Point, Render, SharedString, Styled as _,
    Subscription, Task, Window, anchored, deferred, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, RopeExt as _,
    highlighter::Diagnostic,
    input::{
        CompletionProvider, Copy, Cut, HoverProvider, Input, InputEvent, InputState, Paste,
        SelectAll, TabSize,
    },
    menu::{PopupMenu, PopupMenuItem},
};

use crate::{core::EditorSettings, theme::ApiThemeExt as _};

const DIAGNOSTIC_REFRESH_DEBOUNCE: Duration = Duration::from_millis(120);

/// A syntax-highlighting language understood by `gpui-component`.
///
/// The built-in names below are available when the
/// `gpui-component/tree-sitter-languages` feature is enabled. `Custom` also
/// allows applications to use languages registered through
/// `LanguageRegistry`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CodeLanguage {
    Plain,
    Json,
    JavaScript,
    TypeScript,
    Tsx,
    Html,
    Css,
    Markdown,
    Rust,
    Python,
    Shell,
    Sql,
    GraphQl,
    Yaml,
    Toml,
    Custom(SharedString),
}

impl CodeLanguage {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Plain => "text",
            Self::Json => "json",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Html => "html",
            Self::Css => "css",
            Self::Markdown => "markdown",
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Shell => "bash",
            Self::Sql => "sql",
            Self::GraphQl => "graphql",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Custom(language) => language.as_str(),
        }
    }
}

impl From<&str> for CodeLanguage {
    fn from(language: &str) -> Self {
        Self::Custom(language.to_owned().into())
    }
}

impl From<String> for CodeLanguage {
    fn from(language: String) -> Self {
        Self::Custom(language.into())
    }
}

impl From<SharedString> for CodeLanguage {
    fn from(language: SharedString) -> Self {
        Self::Custom(language)
    }
}

/// Construction options for [`CodeEditor`].
type DiagnosticProvider = Rc<dyn Fn(&str) -> Vec<Diagnostic>>;
type AsyncDiagnosticProvider = Rc<dyn Fn(String, &mut App) -> Task<Vec<Diagnostic>>>;

#[derive(Clone)]
enum DiagnosticProviderKind {
    Synchronous(DiagnosticProvider),
    Asynchronous(AsyncDiagnosticProvider),
}

#[derive(Clone)]
pub struct CodeEditorConfig {
    language: CodeLanguage,
    placeholder: SharedString,
    initial_value: SharedString,
    rows: usize,
    soft_wrap: bool,
    line_numbers: bool,
    read_only: bool,
    auto_close: bool,
    tab_size: TabSize,
    indent_guides: bool,
    framed: bool,
    format_action: bool,
    completion_provider: Option<Rc<dyn CompletionProvider>>,
    hover_provider: Option<Rc<dyn HoverProvider>>,
    diagnostic_provider: Option<DiagnosticProviderKind>,
}

impl Default for CodeEditorConfig {
    fn default() -> Self {
        Self {
            language: CodeLanguage::Json,
            placeholder: SharedString::default(),
            initial_value: SharedString::default(),
            rows: 10,
            soft_wrap: false,
            line_numbers: true,
            read_only: false,
            auto_close: true,
            tab_size: TabSize::default(),
            indent_guides: true,
            framed: true,
            format_action: false,
            completion_provider: None,
            hover_provider: None,
            diagnostic_provider: None,
        }
    }
}

#[allow(dead_code)]
impl CodeEditorConfig {
    pub fn language(mut self, language: impl Into<CodeLanguage>) -> Self {
        self.language = language.into();
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn initial_value(mut self, value: impl Into<SharedString>) -> Self {
        self.initial_value = value.into();
        self
    }

    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows.max(1);
        self
    }

    pub fn soft_wrap(mut self, soft_wrap: bool) -> Self {
        self.soft_wrap = soft_wrap;
        self
    }

    pub fn line_numbers(mut self, line_numbers: bool) -> Self {
        self.line_numbers = line_numbers;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn auto_close(mut self, auto_close: bool) -> Self {
        self.auto_close = auto_close;
        self
    }

    pub fn editor_settings(mut self, settings: &EditorSettings) -> Self {
        self.soft_wrap = settings.soft_wrap;
        self.line_numbers = settings.line_numbers;
        self.auto_close = settings.auto_close_pairs;
        self.tab_size = TabSize {
            tab_size: settings.effective_tab_size(),
            hard_tabs: settings.hard_tabs,
        };
        self.indent_guides = settings.indent_guides;
        self
    }

    pub fn framed(mut self, framed: bool) -> Self {
        self.framed = framed;
        self
    }

    pub fn format_action(mut self, format_action: bool) -> Self {
        self.format_action = format_action;
        self
    }

    pub fn completion_provider(mut self, provider: Rc<dyn CompletionProvider>) -> Self {
        self.completion_provider = Some(provider);
        self
    }

    pub fn hover_provider(mut self, provider: Rc<dyn HoverProvider>) -> Self {
        self.hover_provider = Some(provider);
        self
    }

    pub fn diagnostic_provider(
        mut self,
        provider: impl Fn(&str) -> Vec<Diagnostic> + 'static,
    ) -> Self {
        self.diagnostic_provider = Some(DiagnosticProviderKind::Synchronous(Rc::new(provider)));
        self
    }

    /// Install a diagnostic provider whose work can outlive the current UI
    /// update. The source is owned so the returned task can safely retain it.
    pub fn async_diagnostic_provider(
        mut self,
        provider: impl Fn(String, &mut App) -> Task<Vec<Diagnostic>> + 'static,
    ) -> Self {
        self.diagnostic_provider = Some(DiagnosticProviderKind::Asynchronous(Rc::new(provider)));
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeEditorEvent {
    FormatRequested,
}

/// Reusable syntax-highlighted editor view backed by
/// `gpui_component::input::InputState`.
///
/// `InputEvent`s from the underlying input are re-emitted by this entity, so
/// callers can subscribe to `Entity<CodeEditor>` directly.
#[allow(dead_code)]
pub struct CodeEditor {
    input: Entity<InputState>,
    language: CodeLanguage,
    rows: usize,
    read_only: bool,
    auto_close: bool,
    framed: bool,
    completion_enabled: bool,
    format_action: bool,
    context_menu: Option<Entity<PopupMenu>>,
    context_menu_position: Point<Pixels>,
    diagnostic_provider: Option<DiagnosticProviderKind>,
    diagnostic_refresh_task: Task<()>,
    diagnostic_generation: u64,
    _input_subscription: Subscription,
    _context_menu_subscription: Option<Subscription>,
}

#[allow(dead_code)]
impl CodeEditor {
    pub fn new(config: CodeEditorConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let CodeEditorConfig {
            language,
            placeholder,
            initial_value,
            rows,
            soft_wrap,
            line_numbers,
            read_only,
            auto_close,
            tab_size,
            indent_guides,
            framed,
            format_action,
            completion_provider,
            hover_provider,
            diagnostic_provider,
        } = config;
        let completion_enabled = completion_provider.is_some();

        let highlighter_language: SharedString = language.as_str().to_owned().into();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .code_editor(highlighter_language)
                .rows(rows)
                .soft_wrap(soft_wrap)
                .line_number(line_numbers)
                .tab_size(tab_size)
                .indent_guides(indent_guides);

            if !placeholder.is_empty() {
                state = state.placeholder(placeholder);
            }
            if !initial_value.is_empty() {
                state = state.default_value(initial_value);
            }
            state.lsp.completion_provider = completion_provider;
            state.lsp.hover_provider = hover_provider;
            state
        });

        let input_subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            cx.emit(event.clone());
            if matches!(event, InputEvent::Change) && this.diagnostic_provider.is_some() {
                this.schedule_diagnostic_refresh(cx);
            }
        });

        let mut this = Self {
            input,
            language,
            rows,
            read_only,
            auto_close,
            framed,
            completion_enabled,
            format_action,
            context_menu: None,
            context_menu_position: Point::default(),
            diagnostic_provider,
            diagnostic_refresh_task: Task::ready(()),
            diagnostic_generation: 0,
            _input_subscription: input_subscription,
            _context_menu_subscription: None,
        };
        this.refresh_diagnostics(cx);
        this
    }

    /// Returns the underlying input entity for focus and advanced editor APIs.
    pub fn input_state(&self) -> Entity<InputState> {
        self.input.clone()
    }

    pub fn language(&self) -> &CodeLanguage {
        &self.language
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn value(&self, cx: &App) -> SharedString {
        self.input.read(cx).value()
    }

    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = value.into();
        self.input
            .update(cx, |input, cx| input.set_value(value, window, cx));
        self.refresh_diagnostics(cx);
    }

    pub fn set_language(&mut self, language: impl Into<CodeLanguage>, cx: &mut Context<Self>) {
        let language = language.into();
        let highlighter_language: SharedString = language.as_str().to_owned().into();
        self.language = language;
        self.input.update(cx, |input, cx| {
            input.set_highlighter(highlighter_language, cx)
        });
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx)
        });
    }

    pub fn set_soft_wrap(&mut self, soft_wrap: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_soft_wrap(soft_wrap, window, cx));
    }

    pub fn apply_editor_settings(
        &mut self,
        settings: &EditorSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.auto_close = settings.auto_close_pairs;
        let tab_size = TabSize {
            tab_size: settings.effective_tab_size(),
            hard_tabs: settings.hard_tabs,
        };
        self.input.update(cx, |input, cx| {
            input.set_soft_wrap(settings.soft_wrap, window, cx);
            input.set_line_number(settings.line_numbers, window, cx);
            input.set_indent_guides(settings.indent_guides, window, cx);
            input.set_tab_size(tab_size, window, cx);
        });
        cx.notify();
    }

    pub fn refresh_diagnostics(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.diagnostic_provider.clone() else {
            return;
        };
        let generation = self.next_diagnostic_generation();
        let source = self.input.read(cx).text().to_string();

        match provider {
            DiagnosticProviderKind::Synchronous(provider) => {
                self.diagnostic_refresh_task = Task::ready(());
                let diagnostics = provider(&source);
                self.apply_diagnostics_if_current(generation, diagnostics, cx);
            }
            DiagnosticProviderKind::Asynchronous(provider) => {
                let diagnostics_task = provider(source, cx);
                self.diagnostic_refresh_task = cx.spawn(async move |this, cx| {
                    let diagnostics = diagnostics_task.await;
                    let _ = this.update(cx, |this, cx| {
                        this.apply_diagnostics_if_current(generation, diagnostics, cx);
                    });
                });
            }
        }
    }

    fn schedule_diagnostic_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.diagnostic_provider.clone() else {
            return;
        };
        let generation = self.next_diagnostic_generation();
        let source = self.input.read(cx).text().to_string();

        self.diagnostic_refresh_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(DIAGNOSTIC_REFRESH_DEBOUNCE)
                .await;
            let is_current = this
                .read_with(cx, |this, _| this.diagnostic_generation == generation)
                .unwrap_or(false);
            if !is_current {
                return;
            }

            let diagnostics = match provider {
                DiagnosticProviderKind::Synchronous(provider) => provider(&source),
                DiagnosticProviderKind::Asynchronous(provider) => {
                    let diagnostics_task = match this.update(cx, |this, cx| {
                        (this.diagnostic_generation == generation).then(|| provider(source, cx))
                    }) {
                        Ok(Some(task)) => task,
                        _ => return,
                    };
                    diagnostics_task.await
                }
            };

            let _ = this.update(cx, |this, cx| {
                this.apply_diagnostics_if_current(generation, diagnostics, cx);
            });
        });
    }

    fn next_diagnostic_generation(&mut self) -> u64 {
        self.diagnostic_generation = self.diagnostic_generation.wrapping_add(1);
        self.diagnostic_generation
    }

    fn apply_diagnostics_if_current(
        &mut self,
        generation: u64,
        diagnostics: Vec<Diagnostic>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.diagnostic_generation {
            return;
        }
        self.input.update(cx, |input, cx| {
            let text = input.text().clone();
            if let Some(set) = input.diagnostics_mut() {
                set.reset(&text);
                set.extend(diagnostics);
            }
            cx.notify();
        });
        cx.notify();
    }

    fn capture_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Right || !self.format_action {
            return;
        }

        cx.stop_propagation();
        let position = event.position;
        let (selection, clicked) = self.input.update(cx, |input, cx| {
            (
                EntityInputHandler::selected_text_range(input, true, window, cx)
                    .expect("InputState always provides a selection"),
                EntityInputHandler::character_index_for_point(input, position, window, cx),
            )
        });
        if let Some(clicked_utf16) = clicked
            && !selection.range.contains(&clicked_utf16)
        {
            self.input.update(cx, |input, cx| {
                let offset = input.text().offset_utf16_to_offset(clicked_utf16);
                let point = input.text().offset_to_position(offset);
                input.set_cursor_position(point, window, cx);
            });
        }

        let _ = self.input.update(cx, |input, cx| {
            input.handle_action_for_context_menu(
                Box::new(gpui_component::input::Escape),
                window,
                cx,
            )
        });
        cx.stop_propagation();
        let focus = self.input.read(cx).focus_handle(cx);
        let editor = cx.entity();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            let editor = editor.clone();
            menu.action_context(focus)
                .item(
                    PopupMenuItem::new("Format buffer").on_click(move |_, _, cx| {
                        editor.update(cx, |_, cx| {
                            cx.emit(CodeEditorEvent::FormatRequested);
                        });
                    }),
                )
                .separator()
                .menu("Cut", Box::new(Cut))
                .menu("Copy", Box::new(Copy))
                .menu("Paste", Box::new(Paste))
                .separator()
                .menu("Select All", Box::new(SelectAll))
        });
        let subscription =
            cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, window, cx| {
                this.context_menu = None;
                this.input.read(cx).focus_handle(cx).focus(window);
                cx.notify();
            });
        self.context_menu = Some(menu);
        self.context_menu_position = position;
        self._context_menu_subscription = Some(subscription);
        self.context_menu
            .as_ref()
            .expect("context menu was just installed")
            .read(cx)
            .focus_handle(cx)
            .focus(window);
        cx.notify();
    }

    fn capture_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only || !self.auto_close || event.is_held {
            return;
        }
        if !self.input.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.function {
            return;
        }
        let Some(typed) = event.keystroke.key_char.as_deref() else {
            return;
        };
        if typed.chars().count() != 1 {
            return;
        }
        let typed_char = typed.chars().next().expect("one typed character");
        if !matches!(
            typed_char,
            '(' | ')' | '[' | ']' | '{' | '}' | '\'' | '"' | '`'
        ) {
            return;
        }

        let handled = self.input.update(cx, |input, cx| {
            if EntityInputHandler::marked_text_range(input, window, cx).is_some() {
                return false;
            }
            apply_pair_edit(
                input,
                typed,
                &self.language,
                self.completion_enabled,
                window,
                cx,
            )
        });
        if handled {
            cx.stop_propagation();
        }
    }
}

impl EventEmitter<InputEvent> for CodeEditor {}
impl EventEmitter<CodeEditorEvent> for CodeEditor {}

impl Focusable for CodeEditor {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl Render for CodeEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor_style = cx.api_theme().classes.editor.clone();
        let editor_line_height = editor_style.font_size * (10. / 7.);
        let context_menu = self.context_menu.clone().map(|menu| {
            deferred(
                anchored()
                    .position(self.context_menu_position)
                    .anchor(Corner::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .font_family(cx.theme().font_family.clone())
                            .text_size(cx.theme().font_size)
                            .cursor_default()
                            .child(menu),
                    ),
            )
            .with_priority(1)
        });

        div()
            .id("code-editor")
            .size_full()
            .relative()
            // A code editor is its own scroll surface. Occluding hitboxes
            // behind it keeps an enclosing list or page from handling the
            // same wheel event before InputState consumes it.
            .occlude()
            .when(self.framed, |this| {
                this.rounded_lg()
                    .border_1()
                    .border_color(cx.api_outline_variant())
            })
            .when_some(editor_style.border_radius, |this, radius| {
                this.rounded(radius)
            })
            .mt(editor_style.margin.top)
            .mr(editor_style.margin.right)
            .mb(editor_style.margin.bottom)
            .ml(editor_style.margin.left)
            .pt(editor_style.padding.top)
            .pr(editor_style.padding.right)
            .pb(editor_style.padding.bottom)
            .pl(editor_style.padding.left)
            .bg(cx.api_surface_lowest())
            .overflow_hidden()
            .child(
                Input::new(&self.input)
                    .appearance(false)
                    .disabled(self.read_only)
                    .size_full()
                    .font_family(editor_style.font_family)
                    .text_size(editor_style.font_size)
                    .line_height(editor_line_height),
            )
            .children(context_menu)
            .capture_any_mouse_down(cx.listener(Self::capture_mouse_down))
            .capture_key_down(cx.listener(Self::capture_key_down))
    }
}

fn apply_pair_edit(
    input: &mut InputState,
    typed: &str,
    language: &CodeLanguage,
    completion_enabled: bool,
    window: &mut Window,
    cx: &mut Context<InputState>,
) -> bool {
    if apply_template_pair_edit(input, typed, window, cx) {
        return true;
    }

    let selection = EntityInputHandler::selected_text_range(input, true, window, cx)
        .expect("InputState always provides a selection");
    let cursor = input.cursor();
    let typed_char = typed.chars().next().expect("one typed character");
    let is_quote = matches!(typed_char, '\'' | '"' | '`');
    let has_odd_escape = is_quote && has_odd_escape_prefix(input, cursor);

    if selection.range.is_empty()
        && matches!(typed_char, ')' | ']' | '}' | '\'' | '"' | '`')
        && !has_odd_escape
        && input.text().char_at(cursor) == Some(typed_char)
    {
        let next = cursor + typed.len();
        let point = input.text().offset_to_position(next);
        input.set_cursor_position(point, window, cx);
        return true;
    }

    let Some(close) = closing_delimiter(language, typed_char) else {
        return false;
    };
    if has_odd_escape {
        return false;
    }

    if selection.range.is_empty() {
        if completion_enabled && is_quote {
            // Install the closer silently first, then insert the opener through
            // the platform text path. This leaves the caret inside the pair
            // while preserving completion triggers such as
            // `api.environment.get("`.
            let insertion_range = selection.range;
            input.replace(close.to_string(), window, cx);
            let before_closer = input.text().offset_to_position(cursor);
            input.set_cursor_position(before_closer, window, cx);
            EntityInputHandler::replace_text_in_range(
                input,
                Some(insertion_range),
                typed,
                window,
                cx,
            );
            return true;
        }

        input.replace(format!("{typed}{close}"), window, cx);
        let inside = input.cursor().saturating_sub(close.len_utf8());
        let point = input.text().offset_to_position(inside);
        input.set_cursor_position(point, window, cx);
        return true;
    }

    let mut adjusted = None;
    let selected = EntityInputHandler::text_for_range(
        input,
        selection.range.clone(),
        &mut adjusted,
        window,
        cx,
    )
    .unwrap_or_default();
    EntityInputHandler::replace_text_in_range(
        input,
        Some(selection.range),
        &format!("{typed}{selected}{close}"),
        window,
        cx,
    );
    true
}

/// Handles the language-independent `{{…}}` editing contract used by request
/// templates. On the second opening brace, the missing closing braces are
/// installed silently and the typed brace is inserted through the platform
/// text path so completion providers still receive their trigger.
pub(crate) fn apply_template_pair_edit(
    input: &mut InputState,
    typed: &str,
    window: &mut Window,
    cx: &mut Context<InputState>,
) -> bool {
    let Some(typed_char) = typed.chars().next() else {
        return false;
    };
    if typed.chars().count() != 1 {
        return false;
    }
    if !matches!(typed_char, '{' | '}') {
        return false;
    }

    let selection = EntityInputHandler::selected_text_range(input, true, window, cx)
        .expect("InputState always provides a selection");
    if !selection.range.is_empty() {
        return false;
    }

    let cursor = input.cursor();
    if typed_char == '}' && input.text().char_at(cursor) == Some('}') {
        let point = input.text().offset_to_position(cursor + 1);
        input.set_cursor_position(point, window, cx);
        return true;
    }

    if typed_char != '{'
        || cursor == 0
        || input.text().char_at(cursor.saturating_sub(1)) != Some('{')
    {
        return false;
    }

    let existing_closers = usize::from(input.text().char_at(cursor) == Some('}'))
        + usize::from(input.text().char_at(cursor + 1) == Some('}'));
    let missing_closers = 2usize.saturating_sub(existing_closers);
    if missing_closers > 0 {
        input.replace("}".repeat(missing_closers), window, cx);
        let before_closers = input.text().offset_to_position(cursor);
        input.set_cursor_position(before_closers, window, cx);
    }

    EntityInputHandler::replace_text_in_range(input, Some(selection.range), typed, window, cx);
    true
}

fn closing_delimiter(language: &CodeLanguage, open: char) -> Option<char> {
    use CodeLanguage::*;

    match language {
        Plain | Markdown => None,
        Json => match open {
            '{' => Some('}'),
            '[' => Some(']'),
            '"' => Some('"'),
            _ => None,
        },
        JavaScript | TypeScript | Tsx => match open {
            '{' => Some('}'),
            '[' => Some(']'),
            '(' => Some(')'),
            '\'' => Some('\''),
            '"' => Some('"'),
            '`' => Some('`'),
            _ => None,
        },
        Css | Html | GraphQl | Rust | Sql => match open {
            '{' => Some('}'),
            '[' => Some(']'),
            '(' => Some(')'),
            '\'' => Some('\''),
            '"' => Some('"'),
            _ => None,
        },
        Yaml | Toml => match open {
            '{' => Some('}'),
            '[' => Some(']'),
            '\'' => Some('\''),
            '"' => Some('"'),
            _ => None,
        },
        Shell => match open {
            '{' => Some('}'),
            '[' => Some(']'),
            '(' => Some(')'),
            '\'' => Some('\''),
            '"' => Some('"'),
            '`' => Some('`'),
            _ => None,
        },
        Python => match open {
            '{' => Some('}'),
            '[' => Some(']'),
            '(' => Some(')'),
            _ => None,
        },
        Custom(_) => match open {
            '{' => Some('}'),
            '[' => Some(']'),
            '(' => Some(')'),
            _ => None,
        },
    }
}

fn has_odd_escape_prefix(input: &InputState, cursor: usize) -> bool {
    let mut count = 0;
    for character in input.text().chars_at(cursor).reversed() {
        if character != '\\' {
            break;
        }
        count += 1;
    }
    count % 2 == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        ListAlignment, ListState, Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, div,
        list, point, px, size,
    };
    use gpui_component::Rope;
    use gpui_component::setting::{SettingGroup, SettingItem, SettingPage, Settings};
    use lsp_types::{CompletionContext, CompletionItem, CompletionResponse};
    use std::cell::Cell;

    struct FixedCompletionProvider;

    impl CompletionProvider for FixedCompletionProvider {
        fn supports_inline_completion(&self) -> bool {
            false
        }

        fn completions(
            &self,
            _text: &Rope,
            _offset: usize,
            _trigger: CompletionContext,
            _window: &mut Window,
            _cx: &mut Context<InputState>,
        ) -> Task<anyhow::Result<CompletionResponse>> {
            Task::ready(Ok(CompletionResponse::Array(vec![CompletionItem {
                label: "map".to_owned(),
                ..Default::default()
            }])))
        }

        fn is_completion_trigger(
            &self,
            _offset: usize,
            new_text: &str,
            _cx: &mut Context<InputState>,
        ) -> bool {
            !new_text.is_empty()
                && new_text
                    .chars()
                    .all(|character| character.is_alphanumeric() || matches!(character, '_' | '.'))
        }
    }

    struct NestedScrollView {
        editor: Entity<CodeEditor>,
        outer_scroll: ListState,
    }

    impl Render for NestedScrollView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let editor = self.editor.clone();
            list(self.outer_scroll.clone(), move |index, _, _| {
                if index == 0 {
                    div()
                        .w_full()
                        .h(px(100.))
                        .child(editor.clone())
                        .into_any_element()
                } else {
                    div().w_full().h(px(400.)).into_any_element()
                }
            })
            .size_full()
        }
    }

    struct SettingsEditorView {
        editor: Entity<CodeEditor>,
    }

    impl Render for SettingsEditorView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let editor = self.editor.clone();
            Settings::new("nested-editor-settings")
                .sidebar_width(px(180.))
                .page(
                    SettingPage::new("Appearance")
                        .default_open(true)
                        .resettable(false)
                        .group(
                            SettingGroup::new()
                                .title("CSS editor")
                                .item(SettingItem::render(move |_, _, _| {
                                    div()
                                        .debug_selector(|| "settings-css-editor".to_owned())
                                        .w_full()
                                        .h(px(520.))
                                        .child(editor.clone())
                                })),
                        ),
                )
        }
    }

    fn test_diagnostic(message: &str) -> Diagnostic {
        Diagnostic {
            message: message.to_owned().into(),
            ..Default::default()
        }
    }

    #[gpui::test]
    fn synchronous_diagnostic_provider_remains_immediate(cx: &mut TestAppContext) {
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let code_editor = cx.new(|cx| {
                CodeEditor::new(
                    CodeEditorConfig::default()
                        .initial_value("const value = ;")
                        .diagnostic_provider(|_| vec![test_diagnostic("expected expression")]),
                    window,
                    cx,
                )
            });
            editor = Some(code_editor.clone());
            gpui_component::Root::new(code_editor, window, cx)
        });

        let editor = editor.expect("the window builder installs the editor");
        let input = cx.read(|cx| editor.read(cx).input_state());
        assert_eq!(
            cx.read(|cx| input.read(cx).diagnostics().map_or(0, |set| set.len())),
            1
        );
    }

    #[gpui::test]
    fn async_diagnostics_apply_latest_generation_and_clear(cx: &mut TestAppContext) {
        let calls = Rc::new(Cell::new(0));
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let provider_calls = calls.clone();
            let code_editor = cx.new(|cx| {
                CodeEditor::new(
                    CodeEditorConfig::default()
                        .initial_value("invalid")
                        .async_diagnostic_provider(move |source, _| {
                            provider_calls.set(provider_calls.get() + 1);
                            Task::ready(if source.is_empty() {
                                Vec::new()
                            } else {
                                vec![test_diagnostic("invalid source")]
                            })
                        }),
                    window,
                    cx,
                )
            });
            editor = Some(code_editor.clone());
            gpui_component::Root::new(code_editor, window, cx)
        });
        cx.run_until_parked();

        let editor = editor.expect("the window builder installs the editor");
        let input = cx.read(|cx| editor.read(cx).input_state());
        assert_eq!(calls.get(), 1, "initial construction requests diagnostics");
        assert_eq!(
            cx.read(|cx| input.read(cx).diagnostics().map_or(0, |set| set.len())),
            1
        );

        cx.update(|window, cx| input.read(cx).focus_handle(cx).focus(window));
        cx.simulate_input("abc");
        cx.executor().advance_clock(DIAGNOSTIC_REFRESH_DEBOUNCE);
        cx.run_until_parked();
        assert_eq!(
            calls.get(),
            2,
            "rapid input changes are coalesced into one diagnostic request"
        );
        let stale_generation = cx.read(|cx| editor.read(cx).diagnostic_generation);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| editor.set_value("", window, cx));
        });
        cx.run_until_parked();
        assert!(calls.get() >= 2, "set_value requests fresh diagnostics");
        assert_eq!(
            cx.read(|cx| input.read(cx).diagnostics().map_or(0, |set| set.len())),
            0,
            "an empty latest result clears prior diagnostics"
        );

        editor.update(cx, |editor, cx| {
            editor.apply_diagnostics_if_current(
                stale_generation,
                vec![test_diagnostic("stale")],
                cx,
            );
        });
        assert_eq!(
            cx.read(|cx| input.read(cx).diagnostics().map_or(0, |set| set.len())),
            0,
            "a stale generation must not replace the latest result"
        );
    }

    #[gpui::test]
    fn tab_and_enter_both_confirm_completion(cx: &mut TestAppContext) {
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let code_editor = cx.new(|cx| {
                CodeEditor::new(
                    CodeEditorConfig::default()
                        .language(CodeLanguage::JavaScript)
                        .initial_value("items.")
                        .completion_provider(Rc::new(FixedCompletionProvider)),
                    window,
                    cx,
                )
            });
            editor = Some(code_editor.clone());
            gpui_component::Root::new(code_editor, window, cx)
        });
        let editor = editor.expect("the window builder installs the editor");
        let input = cx.read(|cx| editor.read(cx).input_state());
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_cursor_position(lsp_types::Position::new(0, 6), window, cx);
            });
        });

        cx.simulate_input("ma");
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        assert_eq!(
            cx.read(|cx| input.read(cx).value().to_string()),
            "items.map"
        );

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_value("items.", window, cx);
                input.set_cursor_position(lsp_types::Position::new(0, 6), window, cx);
            });
        });
        cx.simulate_input("ma");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        assert_eq!(
            cx.read(|cx| input.read(cx).value().to_string()),
            "items.map"
        );
    }

    #[gpui::test]
    fn non_triggering_and_silent_edits_invalidate_completion(cx: &mut TestAppContext) {
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let code_editor = cx.new(|cx| {
                CodeEditor::new(
                    CodeEditorConfig::default()
                        .language(CodeLanguage::JavaScript)
                        .initial_value("items.")
                        .completion_provider(Rc::new(FixedCompletionProvider)),
                    window,
                    cx,
                )
            });
            editor = Some(code_editor.clone());
            gpui_component::Root::new(code_editor, window, cx)
        });
        let editor = editor.expect("the window builder installs the editor");
        let input = cx.read(|cx| editor.read(cx).input_state());
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_cursor_position(lsp_types::Position::new(0, 6), window, cx);
            });
        });

        cx.simulate_input("ma");
        cx.run_until_parked();
        cx.simulate_keystrokes("backspace");
        cx.simulate_keystrokes("tab");
        assert_eq!(
            cx.read(|cx| input.read(cx).value().to_string()),
            "items.m ",
            "Backspace must not leave a confirmable completion for old text"
        );

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_value("items.", window, cx);
                input.set_cursor_position(lsp_types::Position::new(0, 6), window, cx);
            });
        });
        cx.simulate_input("ma");
        cx.run_until_parked();
        cx.simulate_input(";");
        cx.simulate_keystrokes("tab");
        assert_eq!(
            cx.read(|cx| input.read(cx).value().to_string()),
            "items.ma; ",
            "non-triggering punctuation must invalidate completion"
        );

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_value("items.ma", window, cx);
                input.set_cursor_position(lsp_types::Position::new(0, 8), window, cx);
            });
        });
        cx.simulate_input("p");
        cx.run_until_parked();
        cx.simulate_keystrokes("alt-backspace");
        assert_eq!(
            cx.read(|cx| input.read(cx).value().to_string()),
            "items.",
            "Option-Backspace must stop at the preceding punctuation boundary"
        );
        cx.simulate_keystrokes("tab");
        assert_eq!(
            cx.read(|cx| input.read(cx).value().to_string()),
            "items.  ",
            "silent editor commands must invalidate completion"
        );
    }

    #[gpui::test]
    fn option_backspace_uses_deletion_boundary_without_changing_option_left(
        cx: &mut TestAppContext,
    ) {
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let code_editor = cx.new(|cx| {
                CodeEditor::new(
                    CodeEditorConfig::default()
                        .language(CodeLanguage::JavaScript)
                        .initial_value("alpha   "),
                    window,
                    cx,
                )
            });
            editor = Some(code_editor.clone());
            gpui_component::Root::new(code_editor, window, cx)
        });
        let editor = editor.expect("the window builder installs the editor");
        let input = cx.read(|cx| editor.read(cx).input_state());
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_cursor_position(lsp_types::Position::new(0, 8), window, cx);
            });
        });

        cx.simulate_keystrokes("alt-backspace");
        assert_eq!(cx.read(|cx| input.read(cx).value().to_string()), "alpha");

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_value("alpha   ", window, cx);
                input.set_cursor_position(lsp_types::Position::new(0, 8), window, cx);
            });
        });
        cx.simulate_keystrokes("alt-left");
        cx.simulate_input("|");
        assert_eq!(
            cx.read(|cx| input.read(cx).value().to_string()),
            "|alpha   ",
            "word navigation remains greedy while deletion narrows whitespace"
        );

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_value("items.map", window, cx);
                input.set_cursor_position(lsp_types::Position::new(0, 9), window, cx);
            });
        });
        cx.simulate_keystrokes("alt-backspace");
        assert_eq!(
            cx.read(|cx| input.read(cx).value().to_string()),
            "items.",
            "Option-Backspace stops at punctuation within a code expression"
        );
    }

    #[gpui::test]
    fn enter_between_brackets_creates_indented_line(cx: &mut TestAppContext) {
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let code_editor = cx.new(|cx| {
                CodeEditor::new(
                    CodeEditorConfig::default()
                        .language(CodeLanguage::JavaScript)
                        .initial_value("{}"),
                    window,
                    cx,
                )
            });
            editor = Some(code_editor.clone());
            gpui_component::Root::new(code_editor, window, cx)
        });
        let editor = editor.expect("the window builder installs the editor");
        let input = cx.read(|cx| editor.read(cx).input_state());
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_cursor_position(lsp_types::Position::new(0, 1), window, cx);
            });
        });

        cx.simulate_keystrokes("enter");
        assert_eq!(cx.read(|cx| input.read(cx).value().to_string()), "{\n  \n}");
        assert_eq!(
            cx.read(|cx| input.read(cx).cursor_position()),
            lsp_types::Position::new(1, 2)
        );
    }

    #[gpui::test]
    fn nested_css_editor_owns_wheel_and_remains_interactive(cx: &mut TestAppContext) {
        let editor_source = format!(
            "/* Unicode must not invalidate highlight byte ranges: → — */\n{}",
            crate::theme::bundled_css()
        );
        let outer_scroll = ListState::new(2, ListAlignment::Top, px(100.));
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let intelligence = Rc::new(crate::theme::ThemeCssIntelligence);
            let code_editor = cx.new(|cx| {
                CodeEditor::new(
                    CodeEditorConfig::default()
                        .language(CodeLanguage::Css)
                        .initial_value(editor_source.clone())
                        .rows(6)
                        .completion_provider(intelligence.clone())
                        .hover_provider(intelligence)
                        .diagnostic_provider(crate::theme::theme_css_diagnostics),
                    window,
                    cx,
                )
            });
            editor = Some(code_editor.clone());
            let view = cx.new(|_| NestedScrollView {
                editor: code_editor,
                outer_scroll: outer_scroll.clone(),
            });
            gpui_component::Root::new(view, window, cx)
        });
        cx.simulate_resize(size(px(240.), px(120.)));
        cx.run_until_parked();

        let editor = editor.expect("the window builder installs the editor");
        let input = cx.read(|cx| editor.read(cx).input_state());
        cx.simulate_click(point(px(80.), px(40.)), Modifiers::none());
        assert!(
            cx.update(|window, cx| input.read(cx).focus_handle(cx).is_focused(window)),
            "the editor must remain focusable inside an occluding scroll surface"
        );
        let cursor_before_scroll = cx.read(|cx| input.read(cx).cursor());

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(40.), px(40.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
            ..Default::default()
        });
        assert_eq!(outer_scroll.logical_scroll_top().item_ix, 0);
        assert_eq!(outer_scroll.logical_scroll_top().offset_in_item, px(0.));

        cx.simulate_click(point(px(80.), px(40.)), Modifiers::none());
        assert!(
            cx.read(|cx| input.read(cx).cursor()) > cursor_before_scroll,
            "scrolling over the editor must move its inner viewport"
        );

        let value_before_typing = cx.read(|cx| input.read(cx).value());
        cx.simulate_input("x");
        assert_ne!(
            cx.read(|cx| input.read(cx).value()),
            value_before_typing,
            "the focused editor must accept text input"
        );

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(40.), px(110.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
            ..Default::default()
        });
        assert!(
            outer_scroll.logical_scroll_top().item_ix > 0
                || outer_scroll.logical_scroll_top().offset_in_item > px(0.)
        );
    }

    #[gpui::test]
    fn css_editor_is_focusable_inside_settings_item(cx: &mut TestAppContext) {
        let editor_source = format!(
            "/* Unicode must not invalidate highlight byte ranges: → — */\n{}",
            crate::theme::bundled_css()
        );
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let intelligence = Rc::new(crate::theme::ThemeCssIntelligence);
            let code_editor = cx.new(|cx| {
                CodeEditor::new(
                    CodeEditorConfig::default()
                        .language(CodeLanguage::Css)
                        .initial_value(editor_source)
                        .rows(28)
                        .completion_provider(intelligence.clone())
                        .hover_provider(intelligence)
                        .diagnostic_provider(crate::theme::theme_css_diagnostics),
                    window,
                    cx,
                )
            });
            editor = Some(code_editor.clone());
            let view = cx.new(|_| SettingsEditorView {
                editor: code_editor,
            });
            gpui_component::Root::new(view, window, cx)
        });
        cx.simulate_resize(size(px(900.), px(700.)));
        cx.run_until_parked();

        let editor_bounds = cx
            .debug_bounds("settings-css-editor")
            .expect("the settings item must lay out its editor");
        assert!(editor_bounds.size.width > px(400.));
        assert_eq!(editor_bounds.size.height, px(520.));

        let editor = editor.expect("the window builder installs the editor");
        let input = cx.read(|cx| editor.read(cx).input_state());
        cx.simulate_click(
            editor_bounds.origin + point(px(80.), px(40.)),
            Modifiers::none(),
        );
        assert!(
            cx.update(|window, cx| input.read(cx).focus_handle(cx).is_focused(window)),
            "the CSS editor must accept pointer focus inside a SettingItem"
        );
        let value_before_typing = cx.read(|cx| input.read(cx).value());
        cx.simulate_input("x");
        assert_ne!(
            cx.read(|cx| input.read(cx).value()),
            value_before_typing,
            "the focused settings CSS editor must accept text input"
        );
    }
}
