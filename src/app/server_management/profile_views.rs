use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use gpui::StatefulInteractiveElement as _;

use super::*;
use crate::core::{ProfileView, SharedHistoryEntry, SharedHistoryHeader, SharedHistoryRequest};

const PROFILE_SIDEBAR_WIDTH: f32 = 250.;
const HISTORY_LIST_WIDTH: f32 = 330.;

pub(super) fn render_profiles(this: &WeakEntity<ApiTester>, cx: &mut App) -> AnyElement {
    let Some(entity) = this.upgrade() else {
        return div().into_any_element();
    };
    let app = entity.read(cx);
    let management = app.server_management.clone();
    let Some(snapshot) = management.snapshot.as_ref() else {
        return profile_message(
            match &management.status {
                ServerManagementStatus::Loading => "Loading server profiles…".to_owned(),
                ServerManagementStatus::Error(error) => error.clone(),
                _ => "Profile data is unavailable.".to_owned(),
            },
            cx,
        );
    };

    let selected = management
        .selected_profile_id
        .as_deref()
        .and_then(|id| snapshot.profiles.iter().find(|profile| profile.id == id))
        .or_else(|| snapshot.profiles.first());
    let rows = snapshot
        .profiles
        .iter()
        .map(|profile| {
            render_profile_row(
                profile,
                selected.is_some_and(|selected| selected.id == profile.id),
                profile.id == snapshot.current_user.id
                    || snapshot.has_permission(HISTORY_READ_OTHERS),
                this,
                cx,
            )
        })
        .collect::<Vec<_>>();

    let sidebar = v_flex()
        .w(px(PROFILE_SIDEBAR_WIDTH))
        .h_full()
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
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{} member profiles", snapshot.profiles.len())),
                ),
        )
        .child(
            v_flex()
                .id("profile-list-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_2()
                .gap_1()
                .children(rows),
        );

    let detail = selected
        .map(|profile| render_profile_detail(profile, snapshot, &management, this, cx))
        .unwrap_or_else(|| profile_message("No profiles are available.".to_owned(), cx));

    h_flex()
        .debug_selector(|| "server-profiles-workspace".to_owned())
        .size_full()
        .min_w_0()
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
    let select_this = this.clone();
    let user_id = profile.id.clone();
    let debug_profile_id = profile.id.clone();
    h_flex()
        .id(SharedString::from(format!("select-profile-{}", profile.id)))
        .debug_selector(move || format!("select-profile-{debug_profile_id}"))
        .w_full()
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

fn render_profile_detail(
    profile: &ProfileView,
    snapshot: &UpstreamManagementSnapshot,
    management: &ServerManagementState,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
    let can_view =
        profile.id == snapshot.current_user.id || snapshot.has_permission(HISTORY_READ_OTHERS);
    let refresh_this = this.clone();
    let refresh_user_id = profile.id.clone();
    let selected_entry = management
        .selected_profile_history_id
        .as_deref()
        .and_then(|id| {
            management
                .profile_history
                .iter()
                .find(|entry| entry.id == id)
        })
        .or_else(|| management.profile_history.first());

    v_flex()
        .size_full()
        .min_w_0()
        .child(
            h_flex()
                .w_full()
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
                                    if profile.id == snapshot.current_user.id {
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
                .child(
                    Button::new(SharedString::from(format!(
                        "refresh-profile-history-{}",
                        profile.id
                    )))
                    .label("Refresh history")
                    .small()
                    .outline()
                    .disabled(
                        !can_view
                            || management.profile_history_status == ProfileHistoryStatus::Loading,
                    )
                    .on_click(move |_, window, cx| {
                        if let Some(this) = refresh_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.load_profile_history(refresh_user_id.clone(), window, cx);
                            });
                        }
                    }),
                ),
        )
        .child(match (can_view, &management.profile_history_status) {
            (false, _) => profile_message(
                "Only members with history.read_others can view this history.".to_owned(),
                cx,
            ),
            (true, ProfileHistoryStatus::Idle) => profile_message(
                "Select this profile or refresh to load its shared history.".to_owned(),
                cx,
            ),
            (true, ProfileHistoryStatus::Loading) => {
                profile_message("Loading shared history…".to_owned(), cx)
            }
            (true, ProfileHistoryStatus::Error(error)) => profile_message(error.clone(), cx),
            (true, ProfileHistoryStatus::Ready) if management.profile_history.is_empty() => {
                profile_message("No shared requests in this workspace yet.".to_owned(), cx)
            }
            (true, ProfileHistoryStatus::Ready) => h_flex()
                .flex_1()
                .min_h_0()
                .items_start()
                .child(render_history_list(management, this, cx))
                .child(
                    div().flex_1().min_w_0().h_full().child(
                        selected_entry
                            .map(|entry| render_history_entry(entry, cx))
                            .unwrap_or_else(|| {
                                profile_message("Select a history entry.".to_owned(), cx)
                            }),
                    ),
                )
                .into_any_element(),
        })
        .into_any_element()
}

fn render_history_list(
    management: &ServerManagementState,
    this: &WeakEntity<ApiTester>,
    cx: &mut App,
) -> AnyElement {
    let rows = management
        .profile_history
        .iter()
        .map(|entry| {
            let select_this = this.clone();
            let entry_id = entry.id.clone();
            let debug_entry_id = entry.id.clone();
            let selected = management.selected_profile_history_id.as_ref() == Some(&entry.id);
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
        .collect::<Vec<_>>();
    v_flex()
        .w(px(HISTORY_LIST_WIDTH))
        .h_full()
        .flex_shrink_0()
        .border_r_1()
        .border_color(cx.api_outline_variant())
        .child(
            h_flex()
                .h(px(42.))
                .px_3()
                .border_b_1()
                .border_color(cx.api_outline_variant())
                .text_xs()
                .font_semibold()
                .child(format!("SHARED HISTORY ({})", rows.len())),
        )
        .child(
            v_flex()
                .id("profile-history-list-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(rows),
        )
        .into_any_element()
}

fn render_history_entry(entry: &SharedHistoryEntry, cx: &mut App) -> AnyElement {
    let response = entry.response.as_ref();
    v_flex()
        .id(SharedString::from(format!(
            "profile-history-entry-scroll-{}",
            entry.id
        )))
        .size_full()
        .min_w_0()
        .overflow_y_scroll()
        .p_5()
        .gap_5()
        .child(
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(method_color(&entry.request.method, cx))
                                .child(entry.request.method.clone()),
                        )
                        .children(response.map(|response| {
                            profile_badge(
                                &format!("{} {}", response.status, response.status_text),
                                status_color(response.status, cx),
                            )
                        })),
                )
                .child(
                    div()
                        .text_sm()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(entry.request.url.clone()),
                ),
        )
        .child(history_headers_section(
            "REQUEST HEADERS",
            &entry.request.headers,
            "Headers marked not to share are omitted.",
            cx,
        ))
        .child(history_request_body_section(&entry.request, cx))
        .when_some(response, |view, response| {
            view.child(history_headers_section(
                "RESPONSE HEADERS",
                &response.headers,
                "Sensitive response headers are redacted before sharing.",
                cx,
            ))
            .child(history_text_section(
                "RESPONSE BODY",
                &shared_response_body(response),
                response.body_truncated,
                cx,
            ))
        })
        .when(!entry.error.is_empty(), |view| {
            view.child(history_text_section("ERROR", &entry.error, false, cx))
        })
        .into_any_element()
}

fn history_headers_section(
    title: &'static str,
    headers: &[SharedHistoryHeader],
    note: &'static str,
    cx: &mut App,
) -> AnyElement {
    v_flex()
        .gap_2()
        .child(history_section_title(title, cx))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(note),
        )
        .child(if headers.is_empty() {
            div()
                .p_3()
                .rounded_md()
                .bg(cx.api_surface_low())
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No shared headers")
                .into_any_element()
        } else {
            v_flex()
                .w_full()
                .rounded_md()
                .border_1()
                .border_color(cx.api_outline_variant())
                .children(headers.iter().enumerate().map(|(index, header)| {
                    h_flex()
                        .gap_4()
                        .px_3()
                        .py_2()
                        .when(index > 0, |row| {
                            row.border_t_1().border_color(cx.api_outline_variant())
                        })
                        .child(
                            div()
                                .w(px(180.))
                                .flex_shrink_0()
                                .text_sm()
                                .font_semibold()
                                .child(header.name.clone()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .text_sm()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(header.value.clone()),
                        )
                }))
                .into_any_element()
        })
        .into_any_element()
}

fn history_request_body_section(request: &SharedHistoryRequest, cx: &mut App) -> AnyElement {
    let body = if request.body_mode == "raw" {
        request.body.clone()
    } else if request.body_fields.is_empty() {
        "No shared request body".to_owned()
    } else {
        request
            .body_fields
            .iter()
            .map(|field| {
                if field.kind == "file" {
                    format!("{} = [file contents and path omitted]", field.name)
                } else {
                    format!("{} = {}", field.name, field.value)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    history_text_section("REQUEST BODY", &body, request.body_truncated, cx)
}

fn history_text_section(
    title: &'static str,
    value: &str,
    truncated: bool,
    cx: &mut App,
) -> AnyElement {
    v_flex()
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .child(history_section_title(title, cx))
                .children(truncated.then(|| profile_badge("Truncated", cx.theme().warning))),
        )
        .child(
            div()
                .id(SharedString::from(format!("profile-history-body-{title}")))
                .debug_selector(move || format!("profile-history-body-{title}"))
                .w_full()
                .max_h(px(360.))
                .overflow_y_scroll()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(cx.api_outline_variant())
                .bg(cx.api_surface_lowest())
                .text_sm()
                .font_family(cx.theme().mono_font_family.clone())
                .child(if value.is_empty() { "Empty" } else { value }.to_owned()),
        )
        .into_any_element()
}

fn shared_response_body(response: &crate::core::SharedHistoryResponse) -> String {
    let Ok(body) = BASE64_STANDARD.decode(&response.body_base64) else {
        return "Invalid shared response body".to_owned();
    };
    String::from_utf8(body.clone())
        .unwrap_or_else(|_| format!("Binary response body ({} bytes)", body.len()))
}

fn history_section_title(title: &'static str, cx: &mut App) -> AnyElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(title)
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
    use gpui::{Context, Render, TestAppContext, Window, px, size};

    use super::*;
    use crate::core::{
        ManagementPermission, ManagementRole, ManagementUser, SharedHistoryResponse,
    };

    struct ProfilesHarness {
        app: Entity<ApiTester>,
    }

    impl Render for ProfilesHarness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            render_profiles(&self.app.downgrade(), cx)
        }
    }

    #[gpui::test]
    fn profiles_render_authorized_shared_request_and_response_history(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("create temporary database directory");
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().expect("initialize test database");

        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let app = cx
                .new(|cx| ApiTester::new_with_database_store(base_key_bindings, store, window, cx));
            let now = Utc::now();
            app.update(cx, |app, _| {
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
                    });
                app.server_management.selected_profile_id = Some("author".to_owned());
                app.server_management.profile_history_status = ProfileHistoryStatus::Ready;
                app.server_management.selected_profile_history_id = Some("entry-1".to_owned());
                app.server_management.profile_history = vec![SharedHistoryEntry {
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
                }];
            });
            let harness = cx.new(|_| ProfilesHarness { app });
            gpui_component::Root::new(harness, window, cx)
        });
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(1_300.), px(900.)));
        cx.run_until_parked();

        assert!(cx.debug_bounds("server-profiles-workspace").is_some());
        assert!(cx.debug_bounds("select-profile-author").is_some());
        assert!(cx.debug_bounds("profile-history-entry-entry-1").is_some());
        assert!(
            cx.debug_bounds("profile-history-body-RESPONSE BODY")
                .is_some()
        );
    }
}
