use std::collections::HashSet;
use std::rc::Rc;

use chrono::Local;
use gpui::{ListState, list};
use serde_json::Value;

use super::*;
use crate::core::{
    AUDIT_READ, ActivityLogEntry, ActivityLogPage, list_audit_activity, list_workspace_activity,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivityLogKind {
    Change,
    Audit,
}

#[derive(Clone, Debug)]
enum ActivityLoadMode {
    Initial,
    Older(String),
    Newer(Option<String>),
}

pub(super) fn render_change_log(
    this: &WeakEntity<ApiTester>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    render_activity_log(this, ActivityLogKind::Change, window, cx)
}

pub(super) fn render_audit_log(
    this: &WeakEntity<ApiTester>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    render_activity_log(this, ActivityLogKind::Audit, window, cx)
}

fn render_activity_log(
    this: &WeakEntity<ApiTester>,
    kind: ActivityLogKind,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    schedule_first_load(this, kind, window, cx);
    let Some(entity) = this.upgrade() else {
        return div().into_any_element();
    };
    let (status, upstream_id, snapshot_present, audit_visible, feed, active_upstream_id, target, realtime_status) = {
        let app = entity.read(cx);
        let management = &app.server_management;
        (
            management.status.clone(),
            management.upstream_id.clone(),
            management.snapshot.is_some(),
            management
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.has_permission(AUDIT_READ)),
            management.activity_feed(kind).clone(),
            app.settings.upstreams.active_upstream_id.clone(),
            app.activity_log_target(kind),
            app.realtime_status,
        )
    };
    if let Some(status_element) = management_status_element(
        &status,
        upstream_id.as_deref(),
        snapshot_present,
        active_upstream_id.as_deref(),
        cx,
    ) {
        return status_element;
    }
    if !snapshot_present {
        return management_empty("Activity log data is unavailable.", cx);
    }
    if kind == ActivityLogKind::Audit && !audit_visible {
        return management_empty(
            "You do not have permission to view the user and role audit log.",
            cx,
        );
    }
    let Ok((upstream_id, workspace_id)) = target else {
        return management_empty(
            if kind == ActivityLogKind::Change {
                "Open a server workspace to view its change log."
            } else {
                "The audit log is unavailable."
            },
            cx,
        );
    };
    let matches_target = feed.upstream_id.as_deref() == Some(upstream_id.as_str())
        && feed.workspace_id == workspace_id;
    let title = if kind == ActivityLogKind::Audit {
        "Identity audit log"
    } else {
        "Workspace change log"
    };
    let description = if kind == ActivityLogKind::Audit {
        "User and role changes, including the exact values that changed."
    } else {
        "Request, collection, and workspace changes in the active workspace."
    };
    let entries = if matches_target {
        feed.entries.as_slice()
    } else {
        &[]
    };
    let loading = !matches_target
        || matches!(
            feed.status,
            ActivityLogStatus::Idle | ActivityLogStatus::Loading
        );
    let initial_error = matches_target
        .then_some(&feed.status)
        .and_then(|status| match status {
            ActivityLogStatus::Error(message) => Some(message.clone()),
            _ => None,
        });
    let load_more_this = this.clone();
    let retry_this = this.clone();
    let realtime_label = match realtime_status {
        RealtimeConnectionStatus::Connected => "Live",
        RealtimeConnectionStatus::Connecting | RealtimeConnectionStatus::Reconnecting => {
            "Connecting"
        }
        RealtimeConnectionStatus::Unavailable => "Realtime unavailable",
        RealtimeConnectionStatus::Inactive => "Realtime inactive",
    };
    let realtime_color = match realtime_status {
        RealtimeConnectionStatus::Connected => cx.theme().success,
        RealtimeConnectionStatus::Connecting | RealtimeConnectionStatus::Reconnecting => {
            cx.theme().warning
        }
        RealtimeConnectionStatus::Unavailable => cx.theme().danger,
        RealtimeConnectionStatus::Inactive => cx.theme().muted_foreground,
    };

    v_flex()
        .id(if kind == ActivityLogKind::Audit {
            "audit-log-workspace"
        } else {
            "change-log-workspace"
        })
        .debug_selector(move || {
            if kind == ActivityLogKind::Audit {
                "audit-log-workspace"
            } else {
                "change-log-workspace"
            }
            .to_owned()
        })
        .size_full()
        .min_h_0()
        .child(
            h_flex()
                .w_full()
                .items_start()
                .justify_between()
                .gap_4()
                .px_5()
                .py_4()
                .border_b_1()
                .border_color(cx.api_outline_variant())
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_1()
                        .child(div().text_lg().font_semibold().child(title))
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(description),
                        ),
                )
                .child(
                    v_flex()
                        .items_end()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().size(px(7.)).rounded_full().bg(realtime_color))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(realtime_color)
                                        .child(realtime_label),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{} loaded", entries.len())),
                        ),
                ),
        )
        .child(if loading {
            management_empty("Loading activity…", cx)
        } else if let Some(error) = initial_error {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .child(div().text_sm().text_color(cx.theme().danger).child(error))
                .child(
                    Button::new(if kind == ActivityLogKind::Audit {
                        "retry-audit-log"
                    } else {
                        "retry-change-log"
                    })
                    .label("Retry")
                    .outline()
                    .on_click(move |_, window, cx| {
                        if let Some(this) = retry_this.upgrade() {
                            this.update(cx, |this, cx| {
                                this.retry_activity_log(kind, window, cx);
                            });
                        }
                    }),
                )
                .into_any_element()
        } else if entries.is_empty() {
            management_empty(
                if kind == ActivityLogKind::Audit {
                    "No user or role changes have been recorded yet."
                } else {
                    "No request, collection, or workspace changes have been recorded yet."
                },
                cx,
            )
        } else {
            let Some(entity) = this.upgrade() else {
                return div().into_any_element();
            };
            // The list virtualizes the feed: only the entries near the visible
            // range are rendered and measured each frame, which keeps scrolling
            // cheap even with many loaded entries.
            let list_state = entity
                .read(cx)
                .server_management
                .activity_feed_list(kind)
                .clone();
            if list_state.item_count() != entries.len() {
                // Safety net: page applications splice precisely; this only
                // fires when the feed was replaced wholesale (target switch).
                list_state.reset(entries.len());
            }
            // The list item builder borrows each visible entry through this
            // shared handle instead of cloning the feed's entries per frame.
            let list_entries = feed.entries.clone();
            v_flex()
                .id(if kind == ActivityLogKind::Audit {
                    "audit-log-scroll"
                } else {
                    "change-log-scroll"
                })
                .debug_selector(move || {
                    if kind == ActivityLogKind::Audit {
                        "audit-log-scroll"
                    } else {
                        "change-log-scroll"
                    }
                    .to_owned()
                })
                .flex_1()
                .min_h_0()
                .when_some(feed.error.clone(), |view, error| {
                    view.child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .border_b_1()
                            .border_color(cx.theme().danger.opacity(0.35))
                            .bg(cx.theme().danger.opacity(0.06))
                            .px_3()
                            .py_2()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(error),
                    )
                })
                .when(feed.syncing, |view| {
                    view.child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .text_center()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Receiving newer changes…"),
                    )
                })
                .child(
                    list(list_state, move |ix, _window, cx| {
                        render_activity_entry(&list_entries[ix], cx)
                    })
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .gap_3(),
                )
                .when(feed.older_cursor.is_some(), |view| {
                    view.child(
                        h_flex()
                            .w_full()
                            .justify_center()
                            .py_2()
                            .flex_shrink_0()
                            .border_t_1()
                            .border_color(cx.api_outline_variant())
                            .child(
                                Button::new(if kind == ActivityLogKind::Audit {
                                    "load-older-audit-log"
                                } else {
                                    "load-older-change-log"
                                })
                                .label(if feed.loading_more {
                                    "Loading…"
                                } else {
                                    "Load older changes"
                                })
                                .outline()
                                .disabled(feed.loading_more || feed.syncing)
                                .on_click(move |_, window, cx| {
                                    if let Some(this) = load_more_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.load_more_activity_log(kind, window, cx);
                                        });
                                    }
                                }),
                        ),
                    )
                })
                .into_any_element()
        })
        .into_any_element()
}

fn schedule_first_load(
    this: &WeakEntity<ApiTester>,
    kind: ActivityLogKind,
    window: &Window,
    cx: &mut App,
) {
    // Only a feed that has never loaded (status Idle) needs a deferred kick;
    // re-renders while it is Loading/Ready must not schedule busywork every
    // frame. Errors are retried explicitly via the Retry button.
    let already_started = this.upgrade().is_some_and(|entity| {
        entity
            .read(cx)
            .server_management
            .activity_feed(kind)
            .status
            != ActivityLogStatus::Idle
    });
    if already_started {
        return;
    }
    let this = this.clone();
    window.defer(cx, move |window, cx| {
        if let Some(this) = this.upgrade() {
            this.update(cx, |this, cx| {
                this.ensure_activity_log_loaded(kind, window, cx);
            });
        }
    });
}

impl ServerManagementState {
    fn activity_feed(&self, kind: ActivityLogKind) -> &ActivityLogFeed {
        match kind {
            ActivityLogKind::Change => &self.change_log,
            ActivityLogKind::Audit => &self.audit_log,
        }
    }

    fn activity_feed_mut(&mut self, kind: ActivityLogKind) -> &mut ActivityLogFeed {
        match kind {
            ActivityLogKind::Change => &mut self.change_log,
            ActivityLogKind::Audit => &mut self.audit_log,
        }
    }

    fn activity_feed_list(&self, kind: ActivityLogKind) -> &ListState {
        match kind {
            ActivityLogKind::Change => &self.change_log_list,
            ActivityLogKind::Audit => &self.audit_log_list,
        }
    }
}

impl ApiTester {
    fn activity_log_target(&self, kind: ActivityLogKind) -> Result<(String, Option<String>), ()> {
        let profile = self.settings.upstreams.active().ok_or(())?;
        if kind == ActivityLogKind::Audit {
            if !self
                .server_management
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.has_permission(AUDIT_READ))
            {
                return Err(());
            }
            return Ok((profile.id.clone(), None));
        }
        let target = self.active_upstream_workspace().map_err(|_| ())?;
        if target.upstream_id != profile.id {
            return Err(());
        }
        Ok((profile.id.clone(), Some(target.workspace_id)))
    }

    fn ensure_activity_log_loaded(
        &mut self,
        kind: ActivityLogKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok((upstream_id, workspace_id)) = self.activity_log_target(kind) else {
            return;
        };
        let feed = self.server_management.activity_feed_mut(kind);
        if feed.upstream_id.as_deref() != Some(upstream_id.as_str())
            || feed.workspace_id != workspace_id
        {
            *feed = ActivityLogFeed {
                upstream_id: Some(upstream_id),
                workspace_id,
                ..ActivityLogFeed::default()
            };
        }
        if matches!(feed.status, ActivityLogStatus::Idle) {
            self.start_activity_log_request(kind, ActivityLoadMode::Initial, window, cx);
        }
    }

    fn retry_activity_log(
        &mut self,
        kind: ActivityLogKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.server_management.activity_feed_mut(kind).status = ActivityLogStatus::Idle;
        self.ensure_activity_log_loaded(kind, window, cx);
    }

    fn load_more_activity_log(
        &mut self,
        kind: ActivityLogKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let feed = self.server_management.activity_feed(kind);
        if feed.loading_more || feed.syncing || !matches!(feed.status, ActivityLogStatus::Ready) {
            return;
        }
        let Some(cursor) = feed.older_cursor.clone() else {
            return;
        };
        self.start_activity_log_request(kind, ActivityLoadMode::Older(cursor), window, cx);
    }

    pub(crate) fn sync_activity_log_realtime(
        &mut self,
        kind: ActivityLogKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok((upstream_id, workspace_id)) = self.activity_log_target(kind) else {
            return;
        };
        let feed = self.server_management.activity_feed_mut(kind);
        if feed.upstream_id.as_deref() != Some(upstream_id.as_str())
            || feed.workspace_id != workspace_id
            || matches!(
                feed.status,
                ActivityLogStatus::Idle | ActivityLogStatus::Error(_)
            )
        {
            return;
        }
        if feed.syncing || feed.loading_more || matches!(feed.status, ActivityLogStatus::Loading) {
            feed.sync_pending = true;
            return;
        }
        let cursor = feed.newer_cursor.clone();
        self.start_activity_log_request(kind, ActivityLoadMode::Newer(cursor), window, cx);
    }

    pub(crate) fn handle_realtime_activity_change(
        &mut self,
        upstream_id: &str,
        change: &RealtimeResourceChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if change.is_identity_change() {
            self.sync_activity_log_realtime(ActivityLogKind::Audit, window, cx);
        }
        if matches!(
            change.resource.as_str(),
            "workspace" | "collection" | "request"
        ) && self.activity_log_target(ActivityLogKind::Change).is_ok_and(
            |(active_upstream_id, workspace_id)| {
                active_upstream_id == upstream_id
                    && workspace_id.as_deref() == change.workspace_id.as_deref()
            },
        ) {
            self.sync_activity_log_realtime(ActivityLogKind::Change, window, cx);
        }
    }

    fn start_activity_log_request(
        &mut self,
        kind: ActivityLogKind,
        mode: ActivityLoadMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok((upstream_id, workspace_id)) = self.activity_log_target(kind) else {
            return;
        };
        let Some(profile) = self.settings.upstreams.server(&upstream_id).cloned() else {
            return;
        };
        let Some(base_url) = profile.parsed_base_url() else {
            return;
        };
        let feed = self.server_management.activity_feed_mut(kind);
        feed.generation = feed.generation.wrapping_add(1);
        let generation = feed.generation;
        feed.error = None;
        match &mode {
            ActivityLoadMode::Initial => feed.status = ActivityLogStatus::Loading,
            ActivityLoadMode::Older(_) => feed.loading_more = true,
            ActivityLoadMode::Newer(_) => {
                feed.syncing = true;
                feed.sync_pending = false;
            }
        }
        let cursor = match &mode {
            ActivityLoadMode::Older(cursor) => Some(cursor.clone()),
            _ => None,
        };
        let after = match &mode {
            ActivityLoadMode::Newer(cursor) => cursor.clone(),
            _ => None,
        };
        let vault = self.credential_vault.clone();
        let runtime = Arc::clone(&self.runtime);
        let task_runtime = Arc::clone(&runtime);
        let client = self.upstream_client.clone();
        let task_upstream_id = upstream_id.clone();
        let task_workspace_id = workspace_id.clone();
        let task = self.runtime.spawn(async move {
            let credential = task_runtime
                .spawn_blocking(move || vault.load_upstream(&task_upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            let result = match kind {
                ActivityLogKind::Change => {
                    list_workspace_activity(
                        &client,
                        &base_url,
                        credential.bearer_token(),
                        task_workspace_id.as_deref().ok_or_else(|| {
                            "Open a server workspace to view its change log.".to_owned()
                        })?,
                        cursor.as_deref(),
                        after.as_deref(),
                    )
                    .await
                }
                ActivityLogKind::Audit => {
                    list_audit_activity(
                        &client,
                        &base_url,
                        credential.bearer_token(),
                        cursor.as_deref(),
                        after.as_deref(),
                    )
                    .await
                }
            };
            result.map_err(|error| error.to_string())
        });
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let target_matches = this
                    .activity_log_target(kind)
                    .is_ok_and(|target| target == (upstream_id.clone(), workspace_id.clone()));
                if !target_matches
                    || this.server_management.activity_feed(kind).generation != generation
                {
                    return;
                }
                let mut continue_sync = false;
                let mut run_pending_sync = false;
                let mut applied = None;
                {
                    let feed = this.server_management.activity_feed_mut(kind);
                    match result {
                        Ok(Ok(page)) => {
                            let has_more_newer = page.has_more_newer;
                            let old_len = feed.entries.len();
                            apply_activity_page(feed, &mode, page);
                            applied = Some((old_len, feed.entries.len()));
                            continue_sync =
                                matches!(mode, ActivityLoadMode::Newer(_)) && has_more_newer;
                            if !continue_sync && feed.sync_pending {
                                feed.sync_pending = false;
                                run_pending_sync = true;
                            }
                        }
                        Ok(Err(error)) => apply_activity_error(feed, &mode, error),
                        Err(error) if error.is_cancelled() => return,
                        Err(error) => apply_activity_error(feed, &mode, error.to_string()),
                    }
                }
                if let Some((old_len, new_len)) = applied {
                    // Keep the virtualized list sized to the feed: reset on a
                    // fresh page, append on "load older", insert on realtime
                    // syncs — instead of letting the render-time safety net
                    // reset (which would yank the scroll back to the top).
                    let list = this.server_management.activity_feed_list(kind).clone();
                    reconcile_feed_list(&list, &mode, old_len, new_len);
                }
                if continue_sync || run_pending_sync {
                    this.sync_activity_log_realtime(kind, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn reconcile_feed_list(list: &ListState, mode: &ActivityLoadMode, old_len: usize, new_len: usize) {
    let grew = new_len.saturating_sub(old_len);
    match mode {
        ActivityLoadMode::Initial => list.reset(new_len),
        ActivityLoadMode::Older(_) => {
            if grew > 0 {
                // Older entries append below the previously loaded page; keep
                // the current scroll position anchored to the same items.
                list.splice(old_len..old_len, grew);
            } else {
                list.reset(new_len);
            }
        }
        ActivityLoadMode::Newer(_) => {
            if grew > 0 {
                // Realtime entries prepend above; the visible items shift by
                // the number of new headers, which the existing sync banner
                // already calls out.
                list.splice(0..0, grew);
            } else {
                list.reset(new_len);
            }
        }
    }
}

fn apply_activity_page(feed: &mut ActivityLogFeed, mode: &ActivityLoadMode, page: ActivityLogPage) {
    match mode {
        ActivityLoadMode::Initial => {
            feed.entries = Rc::new(page.entries);
            feed.older_cursor = page.older_cursor;
            feed.newer_cursor = page.newer_cursor;
            feed.status = ActivityLogStatus::Ready;
        }
        ActivityLoadMode::Older(_) => {
            merge_activity_entries(Rc::make_mut(&mut feed.entries), page.entries);
            feed.older_cursor = page.older_cursor;
            feed.loading_more = false;
        }
        ActivityLoadMode::Newer(cursor) => {
            let was_empty = feed.entries.is_empty();
            merge_activity_entries(Rc::make_mut(&mut feed.entries), page.entries);
            feed.newer_cursor = page.newer_cursor.or_else(|| feed.newer_cursor.clone());
            if cursor.is_none() && was_empty {
                feed.older_cursor = page.older_cursor;
            }
            feed.syncing = false;
        }
    }
    feed.error = None;
}

fn apply_activity_error(feed: &mut ActivityLogFeed, mode: &ActivityLoadMode, error: String) {
    match mode {
        ActivityLoadMode::Initial => feed.status = ActivityLogStatus::Error(error),
        ActivityLoadMode::Older(_) => {
            feed.loading_more = false;
            feed.error = Some(error);
        }
        ActivityLoadMode::Newer(_) => {
            feed.syncing = false;
            feed.error = Some(error);
        }
    }
}

fn merge_activity_entries(entries: &mut Vec<ActivityLogEntry>, incoming: Vec<ActivityLogEntry>) {
    entries.extend(incoming);
    entries.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    let mut seen = HashSet::new();
    entries.retain(|entry| seen.insert(entry.id.clone()));
}

fn render_activity_entry(entry: &ActivityLogEntry, cx: &mut App) -> AnyElement {
    let entry_selector = format!("activity-log-entry-{}", entry.id);
    let actor = if entry.actor_display_name.trim().is_empty() {
        if entry.actor_email.trim().is_empty() {
            "System".to_owned()
        } else {
            entry.actor_email.clone()
        }
    } else {
        entry.actor_display_name.clone()
    };
    let target = if entry.target_name.trim().is_empty() {
        entry.resource_id.clone()
    } else {
        entry.target_name.clone()
    };
    let occurred_at = entry
        .created_at
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let diff_rows = entry
        .diffs
        .iter()
        .map(|diff| render_diff_row(diff, cx))
        .collect::<Vec<_>>();

    v_flex()
        .id(SharedString::from(entry_selector.clone()))
        .debug_selector(move || entry_selector.clone())
        .w_full()
        .flex_shrink_0()
        .rounded_lg()
        .border_1()
        .border_color(cx.api_outline_variant())
        .overflow_hidden()
        .bg(cx.api_surface())
        .child(
            h_flex()
                .w_full()
                .items_start()
                .justify_between()
                .gap_4()
                .px_4()
                .py_3()
                .bg(cx.api_surface_low())
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(activity_badge(&entry.resource, cx.theme().info, cx))
                                .child(activity_badge(&entry.action, cx.theme().warning, cx))
                                .child(div().truncate().text_sm().font_semibold().child(target)),
                        )
                        .child(
                            h_flex()
                                .gap_1()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Changed by")
                                .child(div().font_semibold().child(actor)),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(occurred_at),
                ),
        )
        .when(diff_rows.is_empty(), |view| {
            view.child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(cx.api_outline_variant())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("No effective field changes."),
            )
        })
        .children(diff_rows)
        .into_any_element()
}

fn render_diff_row(diff: &crate::core::ActivityLogDiff, cx: &mut App) -> AnyElement {
    v_flex()
        .w_full()
        .gap_2()
        .px_4()
        .py_3()
        .border_t_1()
        .border_color(cx.api_outline_variant())
        .child(
            div()
                .font_family(cx.theme().mono_font_family.clone())
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(diff.field.clone()),
        )
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap_3()
                .child(diff_value_card("BEFORE", &diff.from, false, cx))
                .child(
                    div()
                        .flex_shrink_0()
                        .pt_5()
                        .text_color(cx.theme().muted_foreground)
                        .child("→"),
                )
                .child(diff_value_card("AFTER", &diff.to, true, cx)),
        )
        .into_any_element()
}

fn diff_value_card(label: &str, value: &Value, after: bool, cx: &mut App) -> AnyElement {
    let text = match value {
        Value::Null => "Not set".to_owned(),
        Value::String(value) if value.is_empty() => "Empty string".to_owned(),
        Value::String(value) => value.clone(),
        _ => serde_json::to_string_pretty(value).unwrap_or_else(|_| "[INVALID VALUE]".to_owned()),
    };
    let color = if after {
        cx.theme().success
    } else {
        cx.theme().danger
    };
    let selector = format!("activity-diff-{}", label.to_ascii_lowercase());
    v_flex()
        .debug_selector(move || selector.clone())
        .min_w_0()
        .flex_1()
        .gap_1()
        .child(
            div()
                .text_size(px(10.))
                .font_semibold()
                .text_color(color)
                .child(label.to_owned()),
        )
        .child(
            div()
                .w_full()
                .min_h(px(38.))
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(color.opacity(0.28))
                .bg(color.opacity(0.055))
                .font_family(cx.theme().mono_font_family.clone())
                .text_sm()
                .whitespace_normal()
                .child(text),
        )
        .into_any_element()
}

fn activity_badge(label: &str, color: Hsla, _cx: &mut App) -> AnyElement {
    div()
        .px_2()
        .py_0p5()
        .rounded_full()
        .border_1()
        .border_color(color.opacity(0.3))
        .bg(color.opacity(0.12))
        .text_color(color)
        .text_xs()
        .font_semibold()
        .child(label.to_ascii_uppercase())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext, Window, px, size};

    use super::*;
    use crate::core::ActivityLogDiff;

    struct ActivityHarness {
        entry: ActivityLogEntry,
    }

    impl Render for ActivityHarness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            v_flex()
                .size_full()
                .p_4()
                .child(render_activity_entry(&self.entry, cx))
        }
    }

    #[gpui::test]
    fn change_log_renders_readable_before_and_after_values(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let now = Utc::now();
            let harness = cx.new(|_| ActivityHarness {
                entry: ActivityLogEntry {
                    id: "entry-1".to_owned(),
                    kind: "change".to_owned(),
                    resource: "request".to_owned(),
                    action: "updated".to_owned(),
                    resource_id: "request-1".to_owned(),
                    workspace_id: "workspace-1".to_owned(),
                    collection_id: "collection-1".to_owned(),
                    actor_user_id: "viewer".to_owned(),
                    actor_email: "viewer@example.test".to_owned(),
                    actor_display_name: "History Viewer".to_owned(),
                    target_name: "Search".to_owned(),
                    diffs: vec![ActivityLogDiff {
                        field: "definition.request.url".to_owned(),
                        from: Value::String("search?q=test".to_owned()),
                        to: Value::String("search?q=test123".to_owned()),
                    }],
                    created_at: now,
                },
            });
            gpui_component::Root::new(harness, window, cx)
        });
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(1_200.), px(800.)));
        cx.run_until_parked();

        assert!(cx.debug_bounds("activity-log-entry-entry-1").is_some());
        assert!(cx.debug_bounds("activity-diff-before").is_some());
        assert!(cx.debug_bounds("activity-diff-after").is_some());
    }

    #[gpui::test]
    fn activity_entries_overflow_the_scroll_list_instead_of_squeezing_into_it(
        cx: &mut TestAppContext,
    ) {
        struct FeedHarness {
            entries: Vec<ActivityLogEntry>,
        }

        impl Render for FeedHarness {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                // Replicates the server-tools settings page chain:
                // SettingPage (v_flex size_full) -> full-bleed body
                // (div flex_1 min_h_0 w_full) -> SettingItem::render_item wrapper
                // (div w_full h_full) -> render_activity_log root (v_flex size_full
                // min_h_0) -> header + bounded overflow_y_scroll feed.
                v_flex()
                    .size_full()
                    .child(div().h(px(46.)).flex_shrink_0())
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .child(
                                div().w_full().h_full().child(
                                    v_flex()
                                        .size_full()
                                        .min_h_0()
                                        .child(div().h(px(58.)).flex_shrink_0())
                                        .child(
                                            v_flex()
                                                .id("feed-scroll")
                                                .debug_selector(|| "feed-scroll".to_owned())
                                                .flex_1()
                                                .min_h_0()
                                                .overflow_y_scroll()
                                                .gap_3()
                                                .p_4()
                                                .children(self.entries.iter().map(|entry| {
                                                    render_activity_entry(entry, cx)
                                                })),
                                        ),
                                ),
                            ),
                    )
            }
        }

        let now = Utc::now();
        let entry = |id: &str, tall: bool| ActivityLogEntry {
            id: id.to_owned(),
            kind: "change".to_owned(),
            resource: "request".to_owned(),
            action: if tall { "updated".to_owned() } else { "created".to_owned() },
            resource_id: format!("{id}-rid"),
            workspace_id: "workspace-1".to_owned(),
            collection_id: "collection-1".to_owned(),
            actor_user_id: "viewer".to_owned(),
            actor_email: "viewer@example.test".to_owned(),
            actor_display_name: "History Viewer".to_owned(),
            target_name: if tall {
                "Login if invalid token".to_owned()
            } else {
                "Small".to_owned()
            },
            diffs: if tall {
                vec![ActivityLogDiff {
                    field: "definition.scripts.pre_request".to_owned(),
                    from: Value::String(
                        "const login = async () => {\n\tconst token = api.environment.get(\"AUTH_TOKEN\")\n\tif (!token) {\n\t\tawait api.requests.execute(ChatAdmin.Auth.Login)\n\t\treturn\n\t}\n}\n\nawait login()"
                            .to_owned(),
                    ),
                    to: Value::String("await login()\napi.request.headers.set(\"Authorization\", \"Bearer \" + api.environment.get(\"AUTH_TOKEN\"))".to_owned()),
                }]
            } else {
                vec![ActivityLogDiff {
                    field: "definition.request.url".to_owned(),
                    from: Value::String("a".to_owned()),
                    to: Value::String("b".to_owned()),
                }]
            },
            created_at: now,
        };

        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let harness = cx.new(|_| FeedHarness {
                entries: (0..20)
                    .map(|i| entry(&format!("entry-{i}"), i % 4 == 0))
                    .collect(),
            });
            gpui_component::Root::new(harness, window, cx)
        });
        cx.update(|window, _| window.activate_window());
        // Small viewport: the combined content is far taller than the feed, so a
        // correctly laid-out feed must overflow (and therefore scroll) instead of
        // flex-shrinking the cards down to fit.
        cx.simulate_resize(size(px(1_300.), px(420.)));
        cx.run_until_parked();

        let feed = cx.debug_bounds("feed-scroll").unwrap();
        let feed_bottom = feed.origin.y + feed.size.height;
        let last = cx.debug_bounds("activity-log-entry-entry-19").unwrap();
        let last_bottom = last.origin.y + last.size.height;
        assert!(
            last_bottom > feed_bottom,
            "last entry ended inside the feed ({last_bottom:?} <= {feed_bottom:?}); entries were squeezed to fit instead of overflowing the scroll list"
        );
    }

    #[gpui::test]
    fn feed_list_virtualizes_offscreen_entries(cx: &mut TestAppContext) {
        struct FeedListHarness {
            entries: Vec<ActivityLogEntry>,
            state: ListState,
        }

        impl Render for FeedListHarness {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let entries = self.entries.clone();
                v_flex()
                    .size_full()
                    .child(
                        list(self.state.clone(), move |ix, _, cx| {
                            render_activity_entry(&entries[ix], cx)
                        })
                        .flex_1()
                        .min_h_0(),
                    )
            }
        }

        let now = Utc::now();
        let entries: Vec<_> = (0..30)
            .map(|i| ActivityLogEntry {
                id: format!("entry-{i}"),
                kind: "change".to_owned(),
                resource: "request".to_owned(),
                action: "updated".to_owned(),
                resource_id: format!("rid-{i}"),
                workspace_id: "workspace-1".to_owned(),
                collection_id: "collection-1".to_owned(),
                actor_user_id: "viewer".to_owned(),
                actor_email: "viewer@example.test".to_owned(),
                actor_display_name: "History Viewer".to_owned(),
                target_name: "Small".to_owned(),
                diffs: vec![ActivityLogDiff {
                    field: "definition.request.url".to_owned(),
                    from: Value::String("a".to_owned()),
                    to: Value::String("b".to_owned()),
                }],
                created_at: now,
            })
            .collect();
        let state = ListState::new(0, gpui::ListAlignment::Top, px(200.));
        state.reset(entries.len());

        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let harness = cx.new(|_| FeedListHarness { entries, state: state.clone() });
            gpui_component::Root::new(harness, window, cx)
        });
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(900.), px(400.)));
        cx.run_until_parked();

        assert_eq!(state.item_count(), 30);
        assert!(
            cx.debug_bounds("activity-log-entry-entry-0").is_some(),
            "top entry should render at the top of the feed"
        );
        assert!(
            cx.debug_bounds("activity-log-entry-entry-29").is_none(),
            "offscreen (bottom) entry must not be laid out — the feed list has to virtualize"
        );
    }
}
