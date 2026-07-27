use std::{rc::Rc, time::Duration};

use gpui::{
    App, AppContext as _, Context, Corner, DismissEvent, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement as _, Pixels, Point, Render, SharedString, Styled as _,
    Subscription, Task, Timer, Window, anchored, deferred, div, px,
};
use gpui_component::{
    ActiveTheme as _, RopeExt as _,
    highlighter::Diagnostic,
    input::{
        CompletionProvider, Copy, Cut, HoverProvider, Input, InputEvent, InputState, Paste,
        SelectAll,
    },
    menu::{PopupMenu, PopupMenuItem},
};

use crate::theme::surface_lowest;

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
    format_action: bool,
    completion_provider: Option<Rc<dyn CompletionProvider>>,
    hover_provider: Option<Rc<dyn HoverProvider>>,
    diagnostic_provider: Option<DiagnosticProvider>,
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
        self.diagnostic_provider = Some(Rc::new(provider));
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
    completion_enabled: bool,
    format_action: bool,
    context_menu: Option<Entity<PopupMenu>>,
    context_menu_position: Point<Pixels>,
    diagnostic_provider: Option<DiagnosticProvider>,
    diagnostic_refresh_task: Task<()>,
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
                .line_number(line_numbers);

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
                this.diagnostic_refresh_task = cx.spawn(async move |this, cx| {
                    Timer::after(DIAGNOSTIC_REFRESH_DEBOUNCE).await;
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |this, cx| this.refresh_diagnostics(cx))
                            .ok();
                    }
                });
            }
        });

        let mut this = Self {
            input,
            language,
            rows,
            read_only,
            auto_close,
            completion_enabled,
            format_action,
            context_menu: None,
            context_menu_position: Point::default(),
            diagnostic_provider,
            diagnostic_refresh_task: Task::ready(()),
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
        self.diagnostic_refresh_task = Task::ready(());
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

    pub fn refresh_diagnostics(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.diagnostic_provider.clone() else {
            return;
        };
        self.input.update(cx, |input, cx| {
            let text = input.text().clone();
            let diagnostics = provider(&text.to_string());
            if let Some(set) = input.diagnostics_mut() {
                set.reset(&text);
                set.extend(diagnostics);
            }
            cx.notify();
        });
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
        let context_menu = self.context_menu.clone().map(|menu| {
            deferred(
                anchored()
                    .position(self.context_menu_position)
                    .anchor(Corner::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .font_family(cx.theme().font_family.clone())
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
            .rounded_lg()
            .border_1()
            .border_color(crate::theme::outline_variant())
            .bg(surface_lowest())
            .overflow_hidden()
            .child(
                Input::new(&self.input)
                    .appearance(false)
                    .disabled(self.read_only)
                    .size_full(),
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
