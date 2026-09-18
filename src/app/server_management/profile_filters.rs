use super::*;
use chrono::{DateTime, NaiveDate, TimeZone};

use crate::core::{ProfileView, SharedHistoryQuery};

const FILTER_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy)]
enum HistoryTextFilter {
    Method,
    Status,
    Hostname,
    Path,
    HeaderKeys,
    ParamKeys,
    From,
    Through,
}

impl HistoryTextFilter {
    const ALL: [Self; 8] = [
        Self::Method,
        Self::Status,
        Self::Hostname,
        Self::Path,
        Self::HeaderKeys,
        Self::ParamKeys,
        Self::From,
        Self::Through,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Method => "HTTP method",
            Self::Status => "Response code",
            Self::Hostname => "Hostname contains",
            Self::Path => "Path contains",
            Self::HeaderKeys => "Request header keys",
            Self::ParamKeys => "Query param keys",
            Self::From => "From date (local)",
            Self::Through => "Through date (local)",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Method => "Any method, e.g. GET",
            Self::Status => "200 / 4xx / error",
            Self::Hostname => "api.example.com",
            Self::Path => "/users",
            Self::HeaderKeys => "content-type, accept",
            Self::ParamKeys => "page, limit",
            Self::From | Self::Through => "YYYY-MM-DD",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Method => "profile-filter-method",
            Self::Status => "profile-filter-status",
            Self::Hostname => "profile-filter-hostname",
            Self::Path => "profile-filter-path",
            Self::HeaderKeys => "profile-filter-header-keys",
            Self::ParamKeys => "profile-filter-param-keys",
            Self::From => "profile-filter-from",
            Self::Through => "profile-filter-through",
        }
    }
}

#[derive(Clone)]
struct ProfileFilterInputs {
    members: Entity<InputState>,
    history: [Entity<InputState>; HistoryTextFilter::ALL.len()],
}

#[derive(Clone, Default)]
pub(super) struct ProfileFiltersState {
    inputs: Option<ProfileFilterInputs>,
    body_type: String,
    sort: String,
    expanded: bool,
    revision: u64,
    pending: bool,
    resume_pending: bool,
    validation_error: Option<String>,
    subscriptions: Rc<Vec<Subscription>>,
}

impl std::fmt::Debug for ProfileFiltersState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileFiltersState")
            .field("body_type", &self.body_type)
            .field("sort", &self.sort)
            .field("expanded", &self.expanded)
            .finish_non_exhaustive()
    }
}

impl ProfileFiltersState {
    pub(super) fn needs_prepare(&self) -> bool {
        self.inputs.is_none() || self.resume_pending
    }

    #[cfg(test)]
    pub(super) fn member_query(&self, cx: &App) -> String {
        self.inputs
            .as_ref()
            .map(|inputs| inputs.members.read(cx).value().trim().to_lowercase())
            .unwrap_or_default()
    }

    fn active_count(&self, cx: &App) -> usize {
        usize::from(!self.body_type.is_empty())
            + self.inputs.as_ref().map_or(0, |inputs| {
                inputs
                    .history
                    .iter()
                    .filter(|input| !input.read(cx).value().trim().is_empty())
                    .count()
            })
    }

    pub(super) fn has_history_filters(&self, cx: &App) -> bool {
        self.active_count(cx) > 0
    }

    fn query(&self, cx: &App) -> Result<SharedHistoryQuery, String> {
        let Some(inputs) = self.inputs.as_ref() else {
            return Ok(SharedHistoryQuery::default());
        };
        let value = |field: HistoryTextFilter| {
            inputs.history[field as usize]
                .read(cx)
                .value()
                .trim()
                .to_owned()
        };
        let method = value(HistoryTextFilter::Method).to_ascii_uppercase();
        if !method.is_empty()
            && (method.len() > 64 || reqwest::Method::from_bytes(method.as_bytes()).is_err())
        {
            return Err("Enter one HTTP method, such as GET or POST.".to_owned());
        }
        let status = normalize_status(&value(HistoryTextFilter::Status))?;
        let (from, before) = date_range(
            &value(HistoryTextFilter::From),
            &value(HistoryTextFilter::Through),
            &Local,
        )?;
        Ok(SharedHistoryQuery {
            method,
            status,
            hostname: value(HistoryTextFilter::Hostname),
            path: value(HistoryTextFilter::Path),
            header_keys: key_list(&value(HistoryTextFilter::HeaderKeys).to_ascii_lowercase()),
            param_keys: key_list(&value(HistoryTextFilter::ParamKeys)),
            body_type: self.body_type.clone(),
            from,
            before,
            sort: self.sort.clone(),
        })
    }
}

pub(super) fn member_matches(profile: &ProfileView, normalized_query: &str) -> bool {
    normalized_query.is_empty()
        || profile
            .display_name
            .to_lowercase()
            .contains(normalized_query)
        || profile.email.to_lowercase().contains(normalized_query)
}

fn key_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalize_status(value: &str) -> Result<String, String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value == "error"
        || matches!(value.as_str(), "1xx" | "2xx" | "3xx" | "4xx" | "5xx")
        || (value.len() == 3
            && value
                .parse::<u16>()
                .is_ok_and(|code| (100..=599).contains(&code)))
    {
        Ok(value)
    } else {
        Err("Response code must be 100–599, a class such as 4xx, or error.".to_owned())
    }
}

/// Date inputs include the entire local day. Convert the next calendar day's
/// midnight, not a fixed 24-hour duration, so daylight-saving transitions work.
fn date_range<T: TimeZone>(
    from: &str,
    through: &str,
    timezone: &T,
) -> Result<(Option<DateTime<Utc>>, Option<DateTime<Utc>>), String> {
    let bound = |value: &str, exclusive_end: bool| -> Result<Option<DateTime<Utc>>, String> {
        if value.trim().is_empty() {
            return Ok(None);
        }
        let label = if exclusive_end { "Through" } else { "From" };
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()
            .filter(|date| date.format("%Y-%m-%d").to_string() == value)
            .ok_or_else(|| format!("{label} date must be a valid YYYY-MM-DD date."))?;
        let date = if exclusive_end {
            date.succ_opt()
                .ok_or_else(|| "Through date is out of range.".to_owned())?
        } else {
            date
        };
        let midnight = date.and_hms_opt(0, 0, 0).expect("midnight is a valid time");
        timezone
            .from_local_datetime(&midnight)
            .earliest()
            .map(|date| Some(date.with_timezone(&Utc)))
            .ok_or_else(|| format!("{label} date has no midnight in the local timezone."))
    };
    let start = bound(from, false)?;
    let end = bound(through, true)?;
    if let (Some(start), Some(end)) = (start, end)
        && start >= end
    {
        return Err("From date must be on or before Through date.".to_owned());
    }
    Ok((start, end))
}

impl ApiTester {
    pub(super) fn ensure_profile_filter_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = &self.server_management.profile_filters;
        if state.inputs.is_some() {
            // A workspace switch cancels an old debounce. Resume an unapplied
            // valid draft only when this server's Profiles view is rendered.
            if state.resume_pending && !state.pending {
                self.queue_profile_history_filter(window, cx);
            }
            return;
        }
        let members = cx.new(|cx| InputState::new(window, cx).placeholder("Search members…"));
        let history = HistoryTextFilter::ALL
            .map(|field| cx.new(|cx| InputState::new(window, cx).placeholder(field.placeholder())));
        let mut subscriptions = vec![cx.subscribe(&members, |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.server_management
                    .profile_lists
                    .search_members(input.read(cx).value().as_ref());
                cx.notify();
            }
        })];
        for input in &history {
            subscriptions.push(
                cx.subscribe_in(input, window, |this, _, event, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.queue_profile_history_filter(window, cx);
                    }
                }),
            );
        }
        let state = &mut self.server_management.profile_filters;
        state.inputs = Some(ProfileFilterInputs { members, history });
        state.subscriptions = Rc::new(subscriptions);
    }

    fn queue_profile_history_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let provider = self.workspace_providers.active_id().clone();
        let state = &mut self.server_management.profile_filters;
        state.revision = state.revision.wrapping_add(1);
        state.pending = false;
        state.resume_pending = false;
        let revision = state.revision;
        let query = match state.query(cx) {
            Ok(query) => {
                state.validation_error = None;
                query
            }
            Err(error) => {
                state.validation_error = Some(error);
                cx.notify();
                return;
            }
        };
        if query == self.server_management.profile_history_query {
            cx.notify();
            return;
        }
        let Some(inputs) = state.inputs.as_ref() else {
            return;
        };
        let input_id = inputs.members.entity_id();
        state.pending = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(FILTER_DEBOUNCE).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let state = &mut this.server_management.profile_filters;
                // An old timer must not change a newly opened server's filters.
                if state.revision != revision
                    || state
                        .inputs
                        .as_ref()
                        .map(|inputs| inputs.members.entity_id())
                        != Some(input_id)
                {
                    return;
                }
                state.pending = false;
                if this.workspace_providers.active_id() != &provider {
                    state.resume_pending = true;
                    cx.notify();
                    return;
                }
                this.server_management.profile_history_query = query;
                if !this.profile_history_view_active() {
                    if let Some(handle) = this.profile_history_abort_handle.take() {
                        handle.abort();
                    }
                    this.profile_history_generation =
                        this.profile_history_generation.wrapping_add(1);
                    this.server_management.reset_profile_history();
                } else if let Some(user_id) = this.server_management.selected_profile_id.clone() {
                    this.load_profile_history(user_id, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn clear_profile_history_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let inputs = self.server_management.profile_filters.inputs.clone();
        let state = &mut self.server_management.profile_filters;
        state.body_type.clear();
        state.sort.clear();
        if let Some(inputs) = inputs {
            for input in inputs.history {
                input.update(cx, |input, cx| input.set_value("", window, cx));
            }
        }
        // Member search is intentionally independent of history filters.
        self.queue_profile_history_filter(window, cx);
    }
}

pub(super) fn render_member_search(state: &ProfileFiltersState) -> AnyElement {
    div()
        .id("profile-member-search")
        .debug_selector(|| "profile-member-search".to_owned())
        .w_full()
        .when_some(state.inputs.as_ref(), |this, inputs| {
            this.child(Input::new(&inputs.members).small().cleanable(true))
        })
        .into_any_element()
}

#[derive(Clone, Copy)]
enum HistoryPicker {
    BodyType,
    Sort,
}

fn picker(
    kind: HistoryPicker,
    state: &ProfileFiltersState,
    this: &WeakEntity<ApiTester>,
) -> AnyElement {
    let (id, selected, options) = match kind {
        HistoryPicker::BodyType => {
            let mut options = vec![
                (String::new(), "Any body type".to_owned()),
                ("none".to_owned(), "No body".to_owned()),
                ("raw".to_owned(), "Raw (any language)".to_owned()),
                (
                    "form_url_encoded".to_owned(),
                    "x-www-form-urlencoded".to_owned(),
                ),
                (
                    "multipart_form_data".to_owned(),
                    "Multipart form-data".to_owned(),
                ),
            ];
            options.extend(RawBodyLanguage::all().iter().map(|language| {
                (
                    format!("raw:{}", language.as_db_str()),
                    format!("Raw · {}", language.label()),
                )
            }));
            ("profile-filter-body-type", state.body_type.clone(), options)
        }
        HistoryPicker::Sort => (
            "profile-history-sort",
            state.sort.clone(),
            [
                ("", "Newest first"),
                ("oldest", "Oldest first"),
                ("duration_asc", "Fastest first"),
                ("duration_desc", "Slowest first"),
                ("status_asc", "Response code ↑"),
                ("status_desc", "Response code ↓"),
                ("method", "Method A–Z"),
                ("hostname", "Hostname A–Z"),
                ("path", "Path A–Z"),
            ]
            .into_iter()
            .map(|(value, label)| (value.to_owned(), label.to_owned()))
            .collect(),
        ),
    };
    let label = options
        .iter()
        .find(|(value, _)| value == &selected)
        .map(|(_, label)| label.clone())
        .unwrap_or_default();
    let menu_this = this.clone();
    Button::new(id)
        .label(label)
        .small()
        .outline()
        .dropdown_caret(true)
        .w_full()
        .dropdown_menu(move |mut menu, _, _| {
            for (value, label) in &options {
                let item_this = menu_this.clone();
                let value = value.clone();
                menu = menu.item(
                    PopupMenuItem::new(label.clone())
                        .checked(value == selected)
                        .on_click(move |_, window, cx| {
                            if let Some(this) = item_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    let state = &mut this.server_management.profile_filters;
                                    match kind {
                                        HistoryPicker::BodyType => state.body_type = value.clone(),
                                        HistoryPicker::Sort => state.sort = value.clone(),
                                    }
                                    this.queue_profile_history_filter(window, cx);
                                });
                            }
                        }),
                );
            }
            menu
        })
        .into_any_element()
}

pub(super) fn render_history_filters(
    state: &ProfileFiltersState,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
    let count = state.active_count(cx);
    let toggle_this = this.clone();
    let clear_this = this.clone();
    v_flex()
        .id("profile-history-filters")
        .debug_selector(|| "profile-history-filters".to_owned())
        .w_full()
        .flex_shrink_0()
        .gap_2()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(cx.api_outline_variant())
        .child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(div().debug_selector(|| "toggle-profile-history-filters".to_owned()).child(
                    Button::new("toggle-profile-history-filters")
                        .label(if count == 0 {
                            "Filter history".to_owned()
                        } else {
                            format!("Filters ({count})")
                        })
                        .icon(if state.expanded {
                            IconName::ChevronUp
                        } else {
                            IconName::ChevronDown
                        })
                        .small()
                        .outline()
                        .selected(state.expanded)
                        .on_click(move |_, _, cx| {
                            if let Some(this) = toggle_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    let state = &mut this.server_management.profile_filters;
                                    state.expanded = !state.expanded;
                                    cx.notify();
                                });
                            }
                        }),
                ))
                .child(
                    div()
                        .id("profile-history-sort-control")
                        .debug_selector(|| "profile-history-sort-control".to_owned())
                        .w(px(178.))
                        .child(picker(HistoryPicker::Sort, state, this)),
                )
                .child(div().debug_selector(|| "clear-profile-history-filters".to_owned()).child(
                    Button::new("clear-profile-history-filters")
                        .label("Clear")
                        .small()
                        .ghost()
                        .disabled(count == 0 && state.sort.is_empty())
                        .on_click(move |_, window, cx| {
                            if let Some(this) = clear_this.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.clear_profile_history_filters(window, cx);
                                });
                            }
                        }),
                ))
                .when(state.pending, |row| {
                    row.child(div().text_xs().text_color(cx.theme().muted_foreground).child("Searching…"))
                }),
        )
        .when(state.expanded, |panel| {
            panel
                .child(
                    h_flex()
                        .items_start()
                        .flex_wrap()
                        .gap_2()
                        .when_some(state.inputs.as_ref(), |row, inputs| {
                            row.children(HistoryTextFilter::ALL.map(|field| {
                                filter_field(field.id(), field.label())
                                    .child(Input::new(&inputs.history[field as usize]).small().cleanable(true))
                            }))
                        })
                        .child(
                            filter_field("profile-filter-body-type-field", "Request body type")
                                .child(picker(HistoryPicker::BodyType, state, this)),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Filters combine. Comma-separated keys must all match; paths and query keys are case-sensitive. Dates include both days."),
                )
        })
        .when_some(state.validation_error.as_ref(), |panel, error| {
            panel.child(
                div()
                    .id("profile-history-filter-error")
                    .debug_selector(|| "profile-history-filter-error".to_owned())
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(format!("{error} Showing the last applied filters.")),
            )
        })
        .into_any_element()
}

fn filter_field(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    v_flex()
        .id(id)
        .debug_selector(move || id.to_owned())
        .w(px(190.))
        .gap_1()
        .child(div().text_xs().child(label))
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn profile_history_filters_debounce_and_cancel_on_workspace_change(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("history-filters.sqlite3"));
        store.initialize().unwrap();
        let mut app_entity = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            let app = cx.new(|cx| ApiTester::new_with_database_store(bindings, store, window, cx));
            app.update(cx, |app, cx| app.ensure_profile_filter_inputs(window, cx));
            app_entity = Some(app.clone());
            Root::new(app, window, cx)
        });
        let app = app_entity.unwrap();
        let inputs = cx.update(|_, cx| {
            app.read(cx)
                .server_management
                .profile_filters
                .inputs
                .clone()
                .unwrap()
        });
        cx.update(|window, cx| {
            inputs.history[HistoryTextFilter::Method as usize]
                .update(cx, |input, cx| input.set_value("get", window, cx));
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(
                app.read(cx)
                    .server_management
                    .profile_history_query
                    .method
                    .is_empty()
            );
            inputs.history[HistoryTextFilter::Method as usize]
                .update(cx, |input, cx| input.set_value("post", window, cx));
            inputs.history[HistoryTextFilter::Status as usize]
                .update(cx, |input, cx| input.set_value("4XX", window, cx));
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(FILTER_DEBOUNCE + Duration::from_millis(1));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let management = &app.read(cx).server_management;
            assert_eq!(management.profile_history_query.method, "POST");
            assert_eq!(management.profile_history_query.status, "4xx");
            assert!(!management.profile_filters.pending);
            // A hidden view applies its draft without downloading body-bearing history.
            assert!(app.read(cx).profile_history_abort_handle.is_none());
            inputs.history[HistoryTextFilter::Method as usize]
                .update(cx, |input, cx| input.set_value("delete", window, cx));
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                app.workspace_providers = WorkspaceProviderRegistry::local(
                    app.database_store.clone(),
                    "another-workspace",
                );
                app.server_management.reset_profile_history();
            });
        });
        cx.executor()
            .advance_clock(FILTER_DEBOUNCE + Duration::from_millis(1));
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(
                app.read(cx).server_management.profile_history_query.method,
                "POST"
            );
            assert!(!app.read(cx).server_management.profile_filters.pending);
            // Showing the same server's view again resumes its unapplied draft.
            app.update(cx, |app, cx| app.ensure_profile_filter_inputs(window, cx));
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(FILTER_DEBOUNCE + Duration::from_millis(1));
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(
                app.read(cx).server_management.profile_history_query.method,
                "DELETE"
            );
            inputs.history[HistoryTextFilter::From as usize]
                .update(cx, |input, cx| input.set_value("2026-02-30", window, cx));
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(
                app.read(cx)
                    .server_management
                    .profile_filters
                    .validation_error
                    .is_some()
            );
            assert!(
                app.read(cx)
                    .server_management
                    .profile_history_query
                    .from
                    .is_none()
            );
        });
    }

    #[test]
    fn member_search_matches_names_and_emails_without_revealing_private_fields() {
        let mut profile = ProfileView {
            id: "member-id".to_owned(),
            display_name: "Request Author".to_owned(),
            email: "author@example.test".to_owned(),
            active: true,
        };
        assert!(member_matches(&profile, "request"));
        assert!(member_matches(&profile, "example.test"));
        assert!(!member_matches(&profile, "other"));
        profile.email.clear();
        assert!(!member_matches(&profile, "author@"));
        assert!(member_matches(&profile, ""));
    }

    #[test]
    fn response_filter_accepts_codes_classes_and_transport_errors() {
        for value in ["", "100", "201", "599", "4xx", "error"] {
            assert_eq!(normalize_status(value).unwrap(), value);
        }
        assert_eq!(normalize_status(" 5XX ").unwrap(), "5xx");
        for value in ["0", "99", "600", "2", "200,404", "success", "2x", "+200"] {
            assert!(normalize_status(value).is_err(), "{value}");
        }
    }

    #[test]
    fn key_filters_trim_empty_and_duplicate_names_but_preserve_case() {
        assert_eq!(
            key_list(" page, ,limit, page, Page "),
            ["Page", "limit", "page"]
        );
    }

    #[test]
    fn request_dates_include_local_days_and_validate_ranges() {
        let offset = FixedOffset::east_opt(9 * 3600).unwrap();
        let (from, before) = date_range("2026-02-03", "2026-02-03", &offset).unwrap();
        assert_eq!(from.unwrap().to_rfc3339(), "2026-02-02T15:00:00+00:00");
        assert_eq!(before.unwrap().to_rfc3339(), "2026-02-03T15:00:00+00:00");
        assert_eq!(date_range("", "", &Utc).unwrap(), (None, None));
        assert!(date_range("2026-02-30", "", &Utc).is_err());
        assert!(date_range("2026-2-3", "", &Utc).is_err());
        assert!(date_range("2026-02-04", "2026-02-03", &Utc).is_err());
        assert!(date_range("", "2026-02-03", &Utc).unwrap().0.is_none());
    }
}
