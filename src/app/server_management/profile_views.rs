use super::*;
use crate::core::{ProfileView, SharedHistoryEntry};
use gpui::{ScrollStrategy, UniformListScrollHandle, uniform_list};

const PROFILE_SIDEBAR_WIDTH: f32 = 250.;
const HISTORY_LIST_WIDTH: f32 = 330.;

#[cfg(test)]
thread_local! {
    static PROFILE_ROWS_RENDERED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static HISTORY_ROWS_RENDERED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Lists own stable, shared snapshots. Searching is an input/data-change cost,
/// not work repeated by every scroll frame or caret blink.
#[derive(Clone, Debug, Default)]
pub(super) struct ProfileLists {
    members: Rc<Vec<ProfileView>>,
    matching_members: Rc<Vec<usize>>,
    member_query: String,
    member_scroll: UniformListScrollHandle,
    history_scroll: UniformListScrollHandle,
}

impl ProfileLists {
    pub(super) fn set_members(&mut self, members: &[ProfileView]) {
        if self.members.as_slice() != members {
            self.members = Rc::new(members.to_vec());
            self.filter_members();
        }
    }

    pub(super) fn search_members(&mut self, query: &str) {
        let query = query.trim().to_lowercase();
        if self.member_query != query {
            self.member_query = query;
            self.filter_members();
            self.member_scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
    }

    fn filter_members(&mut self) {
        self.matching_members = Rc::new(
            self.members
                .iter()
                .enumerate()
                .filter(|(_, member)| profile_filters::member_matches(member, &self.member_query))
                .map(|(index, _)| index)
                .collect(),
        );
    }

    pub(super) fn reset_history_scroll(&mut self) {
        self.history_scroll = UniformListScrollHandle::default();
    }
}

pub(super) fn hide_profiles(this: &WeakEntity<ApiTester>, cx: &mut App) {
    if let Some(entity) = this.upgrade() {
        entity.update(cx, |app, _| {
            app.server_management.profile_history_view_visible = false;
        });
    }
}

pub(super) fn render_profiles(
    this: &WeakEntity<ApiTester>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(entity) = this.upgrade() else {
        return div().into_any_element();
    };
    // Settings stays mounted to retain its selected page and search. Its
    // hidden Profiles page must not construct lists, bodies, or start work.
    if entity.read(cx).workspace_tabs.active() != ActiveWorkspaceTab::ServerTools {
        if entity
            .read(cx)
            .server_management
            .profile_history_view_visible
        {
            hide_profiles(this, cx);
        }
        return div().into_any_element();
    }
    let needs_prepare = {
        let management = &entity.read(cx).server_management;
        !management.profile_history_view_visible
            || management.profile_filters.needs_prepare()
            || (!management.profile_history_syncing
                && (management.profile_history_status == ProfileHistoryStatus::Idle
                    || management.profile_history_refresh_pending))
    };
    if needs_prepare {
        entity.update(cx, |app, cx| {
            if app.server_management.profile_filters.needs_prepare() {
                app.ensure_profile_filter_inputs(window, cx);
            }
            let management = &mut app.server_management;
            management.profile_history_view_visible = true;
            if !management.profile_history_syncing
                && (management.profile_history_status == ProfileHistoryStatus::Idle
                    || management.profile_history_refresh_pending)
            {
                app.ensure_profile_history_loaded(window, cx);
            }
        });
    }
    let (
        status,
        snapshot_present,
        profiles,
        selected_profile_id,
        current_user_id,
        can_view_others,
        history_status,
        history,
        selected_history_id,
        filters,
        realtime_status,
        history_syncing,
        lists,
    ) = {
        let app = entity.read(cx);
        let management = &app.server_management;
        let snapshot = management.snapshot.as_ref();
        let selected_history_id = management
            .selected_profile_history_id
            .as_ref()
            .and_then(|id| {
                management
                    .profile_history
                    .iter()
                    .find(|entry| &entry.id == id)
            })
            .or_else(|| management.profile_history.first())
            .map(|entry| entry.id.clone());
        (
            management.status.clone(),
            snapshot.is_some(),
            management.profile_lists.members.clone(),
            management.selected_profile_id.clone(),
            snapshot
                .map(|snapshot| snapshot.current_user.id.clone())
                .unwrap_or_default(),
            snapshot.is_some_and(|snapshot| snapshot.has_permission(HISTORY_READ_OTHERS)),
            management.profile_history_status.clone(),
            management.profile_history.clone(),
            selected_history_id,
            management.profile_filters.clone(),
            app.realtime_status,
            management.profile_history_syncing,
            management.profile_lists.clone(),
        )
    };
    if !snapshot_present {
        return profile_message(
            match &status {
                ServerManagementStatus::Loading => "Loading server profiles…".to_owned(),
                ServerManagementStatus::Error(error) => error.clone(),
                _ => "Profile data is unavailable.".to_owned(),
            },
            cx,
        );
    }

    let selected = selected_profile_id
        .as_deref()
        .and_then(|id| profiles.iter().find(|profile| profile.id == id))
        .or_else(|| profiles.first());
    let member_query = &lists.member_query;
    let visible_members = lists.matching_members.len();
    let list_members = profiles.clone();
    let matching_members = lists.matching_members.clone();
    let list_selected_id = selected.map(|profile| profile.id.clone());
    let list_current_user_id = current_user_id.clone();
    let list_this = this.clone();

    let sidebar = v_flex()
        .w(px(PROFILE_SIDEBAR_WIDTH))
        .h_full()
        .min_h_0()
        .flex_shrink_0()
        .border_r_1()
        .border_color(cx.api_outline_variant())
        .bg(cx.api_surface_low())
        .child(
            v_flex()
                .gap_1()
                .px_4()
                .py_4()
                .border_b_1()
                .border_color(cx.api_outline_variant())
                .child(div().text_sm().font_semibold().child("SERVER PROFILES"))
                .child(
                    div()
                        .debug_selector(move || format!("profile-member-count-{visible_members}"))
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if member_query.is_empty() {
                            format!("{} member profiles", profiles.len())
                        } else {
                            format!("{visible_members} of {} members", profiles.len())
                        }),
                )
                .child(
                    div()
                        .pt_2()
                        .child(profile_filters::render_member_search(&filters)),
                ),
        )
        .child(div().flex_1().min_h_0().w_full().overflow_hidden().child(
            if visible_members == 0 {
                div()
                    .id("profile-members-empty")
                    .debug_selector(|| "profile-members-empty".to_owned())
                    .p_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(if member_query.is_empty() {
                        "No members available."
                    } else {
                        "No members match your search."
                    })
                    .into_any_element()
            } else {
                uniform_list(
                    "profile-list-scroll",
                    visible_members,
                    move |range, _, cx| {
                        range
                            .map(|index| {
                                let profile = &list_members[matching_members[index]];
                                render_profile_row(
                                    profile,
                                    list_selected_id.as_ref() == Some(&profile.id),
                                    profile.id == list_current_user_id || can_view_others,
                                    &list_this,
                                    cx,
                                )
                            })
                            .collect()
                    },
                )
                .track_scroll(lists.member_scroll.clone())
                .size_full()
                .px_2()
                .into_any_element()
            },
        ));

    let detail = selected
        .map(|profile| {
            render_profile_detail(
                profile,
                &current_user_id,
                can_view_others,
                &history_status,
                &history,
                selected_history_id.as_deref(),
                &filters,
                realtime_status,
                history_syncing,
                &lists.history_scroll,
                this,
                window,
                cx,
            )
        })
        .unwrap_or_else(|| profile_message("No profiles are available.".to_owned(), cx));

    h_flex()
        .debug_selector(|| "server-profiles-workspace".to_owned())
        .size_full()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .items_start()
        .child(sidebar)
        .child(div().flex_1().min_w_0().h_full().child(detail))
        .into_any_element()
}

fn render_profile_row(
    profile: &ProfileView,
    selected: bool,
    can_view_history: bool,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
    #[cfg(test)]
    PROFILE_ROWS_RENDERED.set(PROFILE_ROWS_RENDERED.get() + 1);
    let select_this = this.clone();
    let user_id = profile.id.clone();
    let debug_profile_id = profile.id.clone();
    h_flex()
        .id(SharedString::from(format!("select-profile-{}", profile.id)))
        .debug_selector(move || format!("select-profile-{debug_profile_id}"))
        .w_full()
        .h(rems(3.75))
        .overflow_hidden()
        .gap_2()
        .px_2()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .when(selected, |row| row.bg(cx.theme().sidebar_accent))
        .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.62)))
        .on_click(move |_, window, cx| {
            if let Some(this) = select_this.upgrade() {
                this.update(cx, |this, cx| {
                    this.load_profile_history(user_id.clone(), window, cx);
                });
            }
        })
        .child(profile_avatar(&profile.display_name, cx))
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap_0p5()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_semibold()
                        .child(profile.display_name.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if can_view_history {
                            profile.email.clone()
                        } else {
                            "History private".to_owned()
                        }),
                ),
        )
        .child(div().size(px(7.)).rounded_full().bg(if profile.active {
            cx.theme().success
        } else {
            cx.theme().muted_foreground
        }))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_profile_detail(
    profile: &ProfileView,
    current_user_id: &str,
    can_view_others: bool,
    history_status: &ProfileHistoryStatus,
    history: &Rc<Vec<SharedHistoryEntry>>,
    selected_history_id: Option<&str>,
    filters: &profile_filters::ProfileFiltersState,
    realtime_status: RealtimeConnectionStatus,
    history_syncing: bool,
    history_scroll: &UniformListScrollHandle,
    this: &WeakEntity<ApiTester>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let can_view = profile.id == current_user_id || can_view_others;
    let selected_entry = selected_history_id
        .and_then(|id| history.iter().find(|entry| entry.id == id))
        .or_else(|| history.first());

    v_flex()
        .size_full()
        .min_w_0()
        .min_h_0()
        .child(
            h_flex()
                .w_full()
                .flex_shrink_0()
                .gap_3()
                .p_4()
                .border_b_1()
                .border_color(cx.api_outline_variant())
                .child(profile_avatar_large(&profile.display_name, cx))
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .truncate()
                                        .text_lg()
                                        .font_semibold()
                                        .child(profile.display_name.clone()),
                                )
                                .child(profile_badge(
                                    if profile.id == current_user_id {
                                        "You"
                                    } else if can_view {
                                        "History authorized"
                                    } else {
                                        "History private"
                                    },
                                    if can_view {
                                        cx.theme().info
                                    } else {
                                        cx.theme().muted_foreground
                                    },
                                )),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(profile.email.clone()),
                        ),
                )
                .when(can_view, |header| {
                    header.child(history_live_status(
                        realtime_status,
                        history_syncing,
                        history_status,
                        cx,
                    ))
                }),
        )
        .when(can_view, |detail| {
            detail.child(profile_filters::render_history_filters(filters, this, cx))
        })
        .child(match (can_view, history_status) {
            (false, _) => profile_message(
                "Only members with history.read_others can view this history.".to_owned(),
                cx,
            ),
            (true, ProfileHistoryStatus::Idle) => {
                profile_message("Loading this member's shared history…".to_owned(), cx)
            }
            (true, ProfileHistoryStatus::Loading) => {
                profile_message("Loading shared history…".to_owned(), cx)
            }
            (true, ProfileHistoryStatus::Error(error)) => profile_message(error.clone(), cx),
            (true, ProfileHistoryStatus::Ready) if history.is_empty() => profile_message(
                if filters.has_history_filters(cx) {
                    "No shared requests match these filters. Try changing or clearing the filters."
                } else {
                    "No shared requests in this workspace yet. New requests appear automatically."
                }
                .to_owned(),
                cx,
            ),
            (true, ProfileHistoryStatus::Ready) => h_flex()
                .flex_1()
                .min_h_0()
                .items_start()
                .child(render_history_list(
                    history,
                    selected_history_id,
                    history_scroll,
                    this,
                    cx,
                ))
                .child(
                    div().flex_1().min_w_0().h_full().child(
                        selected_entry
                            .and_then(|entry| this.upgrade().map(|entity| (entity, entry)))
                            .map(|(entity, entry)| {
                                entity.update(cx, |app, cx| {
                                    profile_detail::render_history_entry(
                                        history,
                                        &entry.id,
                                        &mut app.server_management.profile_detail,
                                        window,
                                        cx,
                                    )
                                })
                            })
                            .unwrap_or_else(|| {
                                profile_message("Select a history entry.".to_owned(), cx)
                            }),
                    ),
                )
                .into_any_element(),
        })
        .into_any_element()
}

fn history_live_status(
    connection: RealtimeConnectionStatus,
    syncing: bool,
    status: &ProfileHistoryStatus,
    cx: &mut App,
) -> AnyElement {
    let (label, color) = match connection {
        RealtimeConnectionStatus::Connected => {
            if matches!(status, ProfileHistoryStatus::Error(_)) {
                ("Sync failed", cx.theme().danger)
            } else if syncing || matches!(status, ProfileHistoryStatus::Loading) {
                ("Syncing…", cx.theme().info)
            } else {
                ("Live", cx.theme().success)
            }
        }
        RealtimeConnectionStatus::Connecting => ("Connecting…", cx.theme().muted_foreground),
        RealtimeConnectionStatus::Reconnecting => ("Reconnecting…", cx.theme().muted_foreground),
        RealtimeConnectionStatus::Inactive | RealtimeConnectionStatus::Unavailable => {
            ("Live updates offline", cx.theme().muted_foreground)
        }
    };
    h_flex()
        .id("profile-history-live-status")
        .debug_selector(|| "profile-history-live-status".to_owned())
        .gap_1p5()
        .text_xs()
        .text_color(color)
        .child(div().size(px(6.)).rounded_full().bg(color))
        .child(label)
        .into_any_element()
}

fn render_history_list(
    history: &Rc<Vec<SharedHistoryEntry>>,
    selected_history_id: Option<&str>,
    scroll: &UniformListScrollHandle,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
    let entries = history.clone();
    let this = this.clone();
    let selected_history_id = selected_history_id.map(str::to_owned);
    let render_rows = move |range: std::ops::Range<usize>, _: &mut Window, cx: &mut App| {
        range
            .map(|index| {
                #[cfg(test)]
                HISTORY_ROWS_RENDERED.set(HISTORY_ROWS_RENDERED.get() + 1);
                let entry = &entries[index];
                let select_this = this.clone();
                let entry_id = entry.id.clone();
                let debug_entry_id = entry.id.clone();
                let selected = selected_history_id.as_deref() == Some(entry.id.as_str());
                let status = entry
                    .response
                    .as_ref()
                    .map(|response| response.status.to_string())
                    .unwrap_or_else(|| "ERR".to_owned());
                v_flex()
                    .id(SharedString::from(format!(
                        "profile-history-entry-{}",
                        entry.id
                    )))
                    .debug_selector(move || format!("profile-history-entry-{debug_entry_id}"))
                    .w_full()
                    .h(rems(4.75))
                    .overflow_hidden()
                    .gap_1()
                    .px_3()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .cursor_pointer()
                    .when(selected, |row| row.bg(cx.theme().sidebar_accent))
                    .hover(|style| style.bg(cx.theme().sidebar_accent.opacity(0.62)))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = select_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.server_management.selected_profile_history_id =
                                    Some(entry_id.clone());
                                cx.notify();
                            });
                        }
                    })
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(method_color(&entry.request.method, cx))
                                            .child(entry.request.method.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(status),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        entry
                                            .created_at
                                            .with_timezone(&Local)
                                            .format("%b %d · %H:%M")
                                            .to_string(),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_sm()
                            .child(compact_url(&entry.request.url)),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>()
    };
    v_flex()
        .w(px(HISTORY_LIST_WIDTH))
        .h_full()
        .min_h_0()
        .flex_shrink_0()
        .border_r_1()
        .border_color(cx.api_outline_variant())
        .child(
            v_flex()
                .flex_shrink_0()
                .gap_1()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(cx.api_outline_variant())
                .text_xs()
                .child(
                    div()
                        .font_semibold()
                        .child(format!("SHARED HISTORY ({})", history.len())),
                )
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child("Up to 20 matches · 100 retained per workspace"),
                ),
        )
        .child(
            uniform_list("profile-history-list-scroll", history.len(), render_rows)
                .track_scroll(scroll.clone())
                .flex_1()
                .min_h_0(),
        )
        .into_any_element()
}

fn profile_avatar(name: &str, cx: &mut App) -> AnyElement {
    div()
        .size(px(30.))
        .flex_shrink_0()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(cx.theme().primary.opacity(0.15))
        .text_color(cx.theme().primary)
        .text_xs()
        .font_semibold()
        .child(profile_initials(name))
        .into_any_element()
}

fn profile_avatar_large(name: &str, cx: &mut App) -> AnyElement {
    div()
        .size(px(46.))
        .flex_shrink_0()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(cx.theme().primary.opacity(0.15))
        .text_color(cx.theme().primary)
        .text_sm()
        .font_semibold()
        .child(profile_initials(name))
        .into_any_element()
}

fn profile_initials(name: &str) -> String {
    let initials = name
        .split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>();
    if initials.is_empty() {
        "?".to_owned()
    } else {
        initials.to_uppercase()
    }
}

fn profile_badge(label: &str, color: Hsla) -> AnyElement {
    div()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(color.opacity(0.12))
        .text_color(color)
        .text_xs()
        .font_semibold()
        .child(label.to_owned())
        .into_any_element()
}

fn profile_message(message: String, cx: &mut App) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .p_6()
        .text_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(message)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use gpui::{Context, Modifiers, Render, TestAppContext, Window, px, size};

    use super::*;
    use crate::core::{
        ManagementPermission, ManagementRole, ManagementUser, SharedHistoryHeader,
        SharedHistoryRequest, SharedHistoryResponse,
    };

    struct ProfilesHarness {
        app: Entity<ApiTester>,
    }

    impl Render for ProfilesHarness {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            render_profiles(&self.app.downgrade(), window, cx)
        }
    }

    #[gpui::test]
    fn profiles_render_authorized_shared_request_and_response_history(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("create temporary database directory");
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().expect("initialize test database");

        let mut app_entity = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let app = cx
                .new(|cx| ApiTester::new_with_database_store(base_key_bindings, store, window, cx));
            app_entity = Some(app.clone());
            let now = Utc::now();
            app.update(cx, |app, _| {
                app.workspace_tabs.open_tool(WorkspaceToolTab::ServerTools);
                let permission = ManagementPermission {
                    key: HISTORY_READ_OTHERS.to_owned(),
                    description: "View other users' shared request history".to_owned(),
                };
                let current_user = ManagementUser {
                    id: "viewer".to_owned(),
                    email: "viewer@example.test".to_owned(),
                    display_name: "History Viewer".to_owned(),
                    active: true,
                    roles: vec![ManagementRole {
                        id: "auditor".to_owned(),
                        name: "Auditor".to_owned(),
                        description: String::new(),
                        system: false,
                        permissions: vec![permission],
                        created_by: None,
                        created_at: now,
                        updated_at: now,
                    }],
                    created_by: None,
                    created_at: now,
                    updated_at: now,
                };
                app.server_management
                    .set_snapshot(UpstreamManagementSnapshot {
                        current_user,
                        profiles: vec![
                            ProfileView {
                                id: "viewer".to_owned(),
                                email: "viewer@example.test".to_owned(),
                                display_name: "History Viewer".to_owned(),
                                active: true,
                            },
                            ProfileView {
                                id: "author".to_owned(),
                                email: "author@example.test".to_owned(),
                                display_name: "Request Author".to_owned(),
                                active: true,
                            },
                        ],
                        users: None,
                        roles: None,
                        permissions: None,
                        workspaces: None,
                        request_execution_settings: None,
                        proxies: None,
                    });
                app.server_management.selected_profile_id = Some("author".to_owned());
                app.server_management.profile_history_status = ProfileHistoryStatus::Ready;
                app.server_management.selected_profile_history_id = Some("entry-1".to_owned());
                app.server_management
                    .set_profile_history(vec![SharedHistoryEntry {
                        id: "entry-1".to_owned(),
                        created_at: now,
                        request: SharedHistoryRequest {
                            method: "POST".to_owned(),
                            url: "https://api.example.test/widgets".to_owned(),
                            headers: vec![SharedHistoryHeader {
                                name: "Content-Type".to_owned(),
                                value: "application/json".to_owned(),
                            }],
                            body: r#"{"name":"shared"}"#.to_owned(),
                            body_mode: "raw".to_owned(),
                            raw_body_language: "json".to_owned(),
                            body_fields: Vec::new(),
                            body_truncated: false,
                        },
                        response: Some(SharedHistoryResponse {
                            status: 201,
                            status_text: "Created".to_owned(),
                            http_version: "HTTP/2".to_owned(),
                            final_url: "https://api.example.test/widgets/1".to_owned(),
                            headers: vec![SharedHistoryHeader {
                                name: "Content-Type".to_owned(),
                                value: "application/json".to_owned(),
                            }],
                            body_base64: BASE64_STANDARD.encode(br#"{"id":1}"#),
                            body_truncated: false,
                            content_type: "application/json".to_owned(),
                            duration_micros: 1250,
                        }),
                        error: String::new(),
                    }]);
            });
            let harness = cx.new(|cx| {
                cx.observe(&app, |_, _, cx| cx.notify()).detach();
                ProfilesHarness { app }
            });
            gpui_component::Root::new(harness, window, cx)
        });
        let app = app_entity.unwrap();
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(1_300.), px(900.)));
        cx.run_until_parked();

        assert!(cx.debug_bounds("server-profiles-workspace").is_some());
        assert!(cx.debug_bounds("profile-member-search").is_some());
        assert!(cx.debug_bounds("profile-history-filters").is_some());
        assert!(cx.debug_bounds("profile-history-sort-control").is_some());
        assert!(cx.debug_bounds("profile-history-live-status").is_some());
        assert!(cx.debug_bounds("select-profile-author").is_some());
        assert!(cx.debug_bounds("profile-history-entry-entry-1").is_some());
        assert!(
            cx.debug_bounds("profile-history-body-RESPONSE BODY")
                .is_some()
        );

        // Member search does not disturb the request currently being inspected.
        let search = cx.debug_bounds("profile-member-search").unwrap();
        cx.simulate_click(search.center(), Modifiers::none());
        cx.simulate_input("AUTHOR");
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(
                app.read(cx)
                    .server_management
                    .profile_filters
                    .member_query(cx),
                "author",
            );
        });
        assert!(cx.debug_bounds("select-profile-author").is_some());
        assert!(cx.debug_bounds("profile-member-count-1").is_some());
        assert!(cx.debug_bounds("profile-history-entry-entry-1").is_some());
        cx.simulate_input("-not-found");
        cx.run_until_parked();
        assert!(cx.debug_bounds("profile-members-empty").is_some());
        assert!(cx.debug_bounds("profile-member-count-0").is_some());

        let toggle = cx.debug_bounds("toggle-profile-history-filters").unwrap();
        cx.simulate_click(toggle.center(), Modifiers::none());
        for (width, height) in [(1300., 900.), (1000., 760.)] {
            cx.simulate_resize(size(px(width), px(height)));
            cx.run_until_parked();
            let panel = cx.debug_bounds("profile-history-filters").unwrap();
            for field in [
                "profile-filter-method",
                "profile-filter-status",
                "profile-filter-hostname",
                "profile-filter-path",
                "profile-filter-header-keys",
                "profile-filter-param-keys",
                "profile-filter-from",
                "profile-filter-through",
                "profile-filter-body-type-field",
            ] {
                let bounds = cx.debug_bounds(field).expect(field);
                assert!(bounds.origin.x >= panel.origin.x);
                assert!(
                    bounds.origin.x + bounds.size.width
                        <= panel.origin.x + panel.size.width + px(1.),
                    "{field} overflows at {width}px",
                );
            }
            assert!(cx.debug_bounds("profile-history-entry-entry-1").is_some());
        }

        // Invalid filters are explained locally without replacing valid results.
        let status = cx.debug_bounds("profile-filter-status").unwrap();
        cx.simulate_click(status.center(), Modifiers::none());
        cx.simulate_input("999");
        cx.run_until_parked();
        assert!(cx.debug_bounds("profile-history-filter-error").is_some());
        assert!(cx.debug_bounds("profile-history-entry-entry-1").is_some());
        let clear = cx.debug_bounds("clear-profile-history-filters").unwrap();
        cx.simulate_click(clear.center(), Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, cx| {
            let filters = &app.read(cx).server_management.profile_filters;
            assert!(!filters.has_history_filters(cx));
            assert_eq!(filters.member_query(cx), "author-not-found");
        });
        assert!(cx.debug_bounds("profile-members-empty").is_some());

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let entries = app.server_management.profile_history.clone();
                app.server_management.profile_history_view_visible = false;
                app.refresh_profile_history_realtime("author".into(), window, cx);
                assert!(app.server_management.profile_history_refresh_pending);
                assert!(app.profile_history_abort_handle.is_none());
                assert!(Rc::ptr_eq(&entries, &app.server_management.profile_history));
                app.server_management.profile_history_refresh_pending = false;
                app.catch_up_profile_history_realtime(window, cx);
                assert!(app.server_management.profile_history_refresh_pending);
                assert!(app.profile_history_abort_handle.is_none());

                // A workspace switch can leave the previous management snapshot
                // until the tool is reopened. It must never issue a mixed-server query.
                app.server_management.upstream_id = Some("old-server".into());
                let provider = RemoteWorkspaceProvider::new(
                    app.database_store.clone(),
                    "new-server".into(),
                    "new-workspace".into(),
                    app.workspace.clone(),
                );
                let id = provider.id();
                app.workspace_providers.register(Arc::new(provider));
                app.workspace_providers.switch(id).unwrap();
                let generation = app.profile_history_generation;
                app.load_profile_history("author".into(), window, cx);
                assert_eq!(app.profile_history_generation, generation);
                assert!(app.profile_history_abort_handle.is_none());
                assert!(Rc::ptr_eq(&entries, &app.server_management.profile_history));
            });
        });

        // A large directory and history still construct only a viewport of
        // rows. Count factories, not elapsed time, to make this deterministic.
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let mut snapshot = app.server_management.snapshot.clone().unwrap();
                snapshot
                    .profiles
                    .extend((0..5_000).map(|index| ProfileView {
                        id: format!("bulk-member-{index}"),
                        email: format!("member-{index}@example.test"),
                        display_name: format!("Member {index}"),
                        active: true,
                    }));
                app.server_management.set_snapshot(snapshot);
                app.server_management.profile_lists.search_members("");
                let example = app.server_management.profile_history[0].clone();
                let entries = (0..1_000)
                    .map(|index| {
                        let mut entry = example.clone();
                        entry.id = format!("bulk-entry-{index}");
                        entry
                    })
                    .collect();
                app.server_management.set_profile_history(entries);
                app.server_management.profile_history_refresh_pending = false;
                app.server_management.profile_history_status = ProfileHistoryStatus::Ready;
                cx.notify();
            });
        });
        cx.run_until_parked();
        PROFILE_ROWS_RENDERED.set(0);
        HISTORY_ROWS_RENDERED.set(0);
        cx.update(|_, cx| app.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        let member_rows = PROFILE_ROWS_RENDERED.get();
        let history_rows = HISTORY_ROWS_RENDERED.get();
        assert!(
            member_rows > 0 && member_rows < 100,
            "{member_rows} member rows rendered"
        );
        assert!(
            history_rows > 0 && history_rows < 100,
            "{history_rows} history rows rendered"
        );
        assert!(cx.debug_bounds("select-profile-bulk-member-4999").is_none());
        assert!(
            cx.debug_bounds("profile-history-entry-bulk-entry-999")
                .is_none()
        );

        // No periodic filter parsing/refetching or parent invalidation after
        // the view settles, even while an input is focused and its caret blinks.
        let search = cx.debug_bounds("profile-member-search").unwrap();
        cx.simulate_click(search.center(), Modifiers::none());
        cx.run_until_parked();
        PROFILE_ROWS_RENDERED.set(0);
        HISTORY_ROWS_RENDERED.set(0);
        for _ in 0..10 {
            cx.executor().advance_clock(Duration::from_millis(500));
            cx.run_until_parked();
        }
        assert_eq!(PROFILE_ROWS_RENDERED.get(), 0);
        assert_eq!(HISTORY_ROWS_RENDERED.get(), 0);

        // Keeping Server Tools open while another tab is active must preserve
        // state without even constructing its hidden member/history rows.
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.workspace_tabs.activate_request();
                cx.notify();
            })
        });
        cx.run_until_parked();
        PROFILE_ROWS_RENDERED.set(0);
        HISTORY_ROWS_RENDERED.set(0);
        for _ in 0..10 {
            cx.update(|_, cx| app.update(cx, |_, cx| cx.notify()));
            cx.run_until_parked();
        }
        assert_eq!(PROFILE_ROWS_RENDERED.get(), 0);
        assert_eq!(HISTORY_ROWS_RENDERED.get(), 0);
        eprintln!(
            "Profiles fixture: 5,002 members / 1,000 history entries; visible frame built {member_rows} / {history_rows} rows; settled idle and hidden built zero."
        );
    }
}
