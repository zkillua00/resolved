use super::*;

/// A complete-list edit, kept separate from the live snapshot. Only a save started
/// by this editor may close it on mutation completion.
#[derive(Clone, Debug, Default)]
pub(super) struct ProxyScopeEditorState {
    draft: Option<ScopeDraft>,
    saving: bool,
    save_error: Option<String>,
}

#[derive(Clone, Debug)]
struct ScopeDraft {
    proxy_id: String,
    proxy_name: String,
    base: ScopeSelection,
    selection: ScopeSelection,
    conflict: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ScopeSelection {
    Assignments(Vec<ProxyAssignment>),
    Exclusions {
        users: Vec<String>,
        roles: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ScopeItem {
    Assignment(ProxyAssignment),
    User(String),
    Role(String),
}

impl ScopeSelection {
    fn from_proxy(proxy: &ManagementProxy, assignments: bool) -> Self {
        if assignments {
            Self::Assignments(proxy.assignments.clone())
        } else {
            Self::Exclusions {
                users: proxy.excluded_user_ids.clone(),
                roles: proxy.excluded_role_ids.clone(),
            }
        }
    }

    fn is_assignments(&self) -> bool {
        matches!(self, Self::Assignments(_))
    }

    fn contains(&self, item: &ScopeItem) -> bool {
        match (self, item) {
            (Self::Assignments(values), ScopeItem::Assignment(value)) => values.contains(value),
            (Self::Exclusions { users, .. }, ScopeItem::User(id)) => users.contains(id),
            (Self::Exclusions { roles, .. }, ScopeItem::Role(id)) => roles.contains(id),
            _ => false,
        }
    }

    fn set(&mut self, item: &ScopeItem, checked: bool) {
        match (self, item) {
            (Self::Assignments(values), ScopeItem::Assignment(value)) => {
                set_selected(values, value, checked);
            }
            (Self::Exclusions { users, .. }, ScopeItem::User(id)) => {
                set_selected(users, id, checked);
            }
            (Self::Exclusions { roles, .. }, ScopeItem::Role(id)) => {
                set_selected(roles, id, checked);
            }
            _ => {}
        }
    }

    // Lists are sets on the wire; a harmless server ordering change is not a conflict.
    fn same_members(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Assignments(a), Self::Assignments(b)) => same_members(a, b),
            (
                Self::Exclusions {
                    users: a,
                    roles: ar,
                },
                Self::Exclusions {
                    users: b,
                    roles: br,
                },
            ) => same_members(a, b) && same_members(ar, br),
            _ => false,
        }
    }
}

fn same_members<T: PartialEq>(a: &[T], b: &[T]) -> bool {
    a.iter().all(|value| b.contains(value)) && b.iter().all(|value| a.contains(value))
}

fn set_selected<T: Clone + PartialEq>(values: &mut Vec<T>, value: &T, checked: bool) {
    values.retain(|entry| entry != value);
    if checked {
        values.push(value.clone());
    }
}

impl ProxyScopeEditorState {
    pub(super) fn is_editing(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    fn begin(&mut self, proxy: &ManagementProxy, assignments: bool) {
        let base = ScopeSelection::from_proxy(proxy, assignments);
        self.draft = Some(ScopeDraft {
            proxy_id: proxy.id.clone(),
            proxy_name: proxy.name.clone(),
            selection: base.clone(),
            base,
            conflict: false,
        });
        self.saving = false;
        self.save_error = None;
    }

    pub(super) fn reconcile(&mut self, snapshot: &UpstreamManagementSnapshot) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        let proxy = snapshot
            .proxies
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|proxy| proxy.id == draft.proxy_id);
        // Sticky conflict: Cancel and reopen is an explicit reload. Never silently
        // replace someone else's complete list, even if it later changes back.
        draft.conflict |= proxy.is_none_or(|proxy| {
            !draft.base.same_members(&ScopeSelection::from_proxy(
                proxy,
                draft.base.is_assignments(),
            ))
        });
        if let ScopeSelection::Assignments(values) = &draft.selection {
            draft.conflict |= values
                .iter()
                .any(|value| assignment_owner(snapshot, &draft.proxy_id, value).is_some());
        }
    }

    pub(super) fn finish_save(&mut self, success: bool) {
        if self.saving {
            self.saving = false;
            if success {
                self.clear();
            }
        }
    }

    pub(super) fn set_save_error(&mut self, error: String) {
        if self.is_editing() {
            self.save_error = Some(error);
        }
    }
}

#[derive(Clone, Debug)]
struct ScopeOption {
    assignment: ProxyAssignment,
    label: String,
}

fn scope_kind_label(kind: ProxyScopeKind) -> &'static str {
    match kind {
        ProxyScopeKind::Server => "Server",
        ProxyScopeKind::Workspace => "Workspace",
        ProxyScopeKind::Collection => "Collection",
        ProxyScopeKind::Request => "Saved request",
    }
}

fn scope_options(workspaces: Option<&[UpstreamWorkspaceView]>) -> Vec<ScopeOption> {
    let mut options = vec![ScopeOption {
        assignment: ProxyAssignment {
            scope_kind: ProxyScopeKind::Server,
            scope_id: None,
        },
        label: "Server-wide · All workspaces".into(),
    }];
    for workspace in workspaces.unwrap_or_default() {
        options.push(ScopeOption {
            assignment: ProxyAssignment {
                scope_kind: ProxyScopeKind::Workspace,
                scope_id: Some(workspace.id.clone()),
            },
            label: format!("Workspace · {}", workspace.name),
        });
        for collection in &workspace.collections {
            push_collection_options(&mut options, collection, &workspace.name);
        }
    }
    options
}

fn push_collection_options(
    options: &mut Vec<ScopeOption>,
    collection: &UpstreamCollectionView,
    parent_path: &str,
) {
    let path = format!("{parent_path} / {}", collection.name);
    options.push(ScopeOption {
        assignment: ProxyAssignment {
            scope_kind: ProxyScopeKind::Collection,
            scope_id: Some(collection.id.clone()),
        },
        label: format!("Collection · {path}"),
    });
    for request in &collection.requests {
        options.push(ScopeOption {
            assignment: ProxyAssignment {
                scope_kind: ProxyScopeKind::Request,
                scope_id: Some(request.id.clone()),
            },
            label: format!("Saved request · {path} / {}", request.name),
        });
    }
    for child in &collection.sub_collections {
        push_collection_options(options, child, &path);
    }
}

fn inaccessible_assignment_label(assignment: &ProxyAssignment) -> String {
    format!(
        "Inaccessible {} · ID: {}",
        scope_kind_label(assignment.scope_kind),
        assignment.scope_id.as_deref().unwrap_or("(unavailable)")
    )
}

pub(super) fn proxy_assignment_label(
    assignment: &ProxyAssignment,
    workspaces: Option<&[UpstreamWorkspaceView]>,
) -> String {
    scope_options(workspaces)
        .into_iter()
        .find(|option| &option.assignment == assignment)
        .map(|option| option.label)
        .unwrap_or_else(|| inaccessible_assignment_label(assignment))
}

fn assignment_owner<'a>(
    snapshot: &'a UpstreamManagementSnapshot,
    proxy_id: &str,
    assignment: &ProxyAssignment,
) -> Option<&'a str> {
    snapshot
        .proxies
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|proxy| proxy.id != proxy_id && proxy.assignments.contains(assignment))
        .map(|proxy| proxy.name.as_str())
}

struct SelectionRow {
    item: ScopeItem,
    label: String,
    owner: Option<String>,
}

/// Include the baseline as well as the selection: an inaccessible item that was
/// unchecked must remain visible so that the user can undo that change.
fn selection_rows(
    proxy: &ManagementProxy,
    snapshot: &UpstreamManagementSnapshot,
    selection: &ScopeSelection,
    base: &ScopeSelection,
    editing: bool,
) -> Vec<SelectionRow> {
    let mut rows = Vec::new();
    match selection {
        ScopeSelection::Assignments(values) => {
            let mut options = scope_options(snapshot.workspaces.as_deref());
            for value in values.iter().chain(match base {
                ScopeSelection::Assignments(base) => base.as_slice(),
                _ => &[],
            }) {
                if !options.iter().any(|option| &option.assignment == value) {
                    options.push(ScopeOption {
                        assignment: value.clone(),
                        label: inaccessible_assignment_label(value),
                    });
                }
            }
            for option in options {
                if editing || values.contains(&option.assignment) {
                    rows.push(SelectionRow {
                        owner: assignment_owner(snapshot, &proxy.id, &option.assignment)
                            .map(str::to_owned),
                        item: ScopeItem::Assignment(option.assignment),
                        label: option.label,
                    });
                }
            }
        }
        ScopeSelection::Exclusions { users, roles } => {
            for is_user in [false, true] {
                let known: Vec<(String, String)> = if is_user {
                    snapshot
                        .users
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .map(|user| (user.id.clone(), user.display_name.clone()))
                        .collect()
                } else {
                    snapshot
                        .roles
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .map(|role| (role.id.clone(), role.name.clone()))
                        .collect()
                };
                let selected = if is_user { users } else { roles };
                let baseline = match base {
                    ScopeSelection::Exclusions { users, roles } => {
                        if is_user {
                            users
                        } else {
                            roles
                        }
                    }
                    _ => selected,
                };
                let noun = if is_user { "User" } else { "Role" };
                let mut subjects = known;
                for id in selected.iter().chain(baseline) {
                    if !subjects.iter().any(|(known_id, _)| known_id == id) {
                        subjects.push((id.clone(), format!("Inaccessible · ID: {id}")));
                    }
                }
                for (id, name) in subjects {
                    if editing || selected.contains(&id) {
                        rows.push(SelectionRow {
                            item: if is_user {
                                ScopeItem::User(id)
                            } else {
                                ScopeItem::Role(id)
                            },
                            label: format!("{noun} · {name}"),
                            owner: None,
                        });
                    }
                }
            }
        }
    }
    rows
}

impl ApiTester {
    /// Render only information captured by the draft, never a substituted proxy.
    pub(super) fn render_orphaned_proxy_scope_editor(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let editor = &self.server_management.proxy_scope_editor;
        let draft = editor.draft.as_ref()?;
        let count = match &draft.selection {
            ScopeSelection::Assignments(values) => values.len(),
            ScopeSelection::Exclusions { users, roles } => users.len() + roles.len(),
        };
        Some(v_flex()
            .debug_selector(|| "proxy-scope-orphaned-editor".to_owned())
            .flex_1().w_full().min_h_0()
            .child(v_flex().id("proxy-scope-orphaned-scroll")
                .flex_1().min_h_0().overflow_y_scroll().gap_3().p_5()
                .child(div().text_lg().font_semibold()
                    .child(format!("{} · ID: {}", draft.proxy_name, draft.proxy_id)))
                .child(scope_note("This proxy is unavailable or no longer readable. Your draft is preserved, but cannot be saved. Cancel to discard it and review the available proxies.", cx))
                .child(div().text_sm().child(format!(
                    "{} draft: {count} selected. Selections are preserved locally.",
                    if draft.selection.is_assignments() { "Assignments" } else { "Exclusions" },
                ))))
            .child(h_flex().flex_shrink_0().gap_2().p_5()
                .border_t_1().border_color(cx.api_outline_variant())
                .debug_selector(|| "proxy-scope-footer".to_owned())
                .child(div().debug_selector(|| "proxy-scope-save".to_owned()).child(
                    Button::new("proxy-scope-save").label("Save").small().primary().disabled(true)))
                .child(self.render_proxy_scope_cancel(cx)))
            .into_any_element())
    }

    fn render_proxy_scope_cancel(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .debug_selector(|| "proxy-scope-cancel".to_owned())
            .child(
                Button::new("proxy-scope-cancel")
                    .label("Cancel")
                    .small()
                    .outline()
                    .disabled(self.server_management.proxy_scope_editor.saving)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.server_management.proxy_scope_editor.saving {
                            this.server_management.proxy_scope_editor.clear();
                            cx.notify();
                        }
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_proxy_assignments_tab(
        &self,
        proxy: &ManagementProxy,
        snapshot: &UpstreamManagementSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_proxy_scope_tab(proxy, snapshot, true, cx)
    }

    pub(super) fn render_proxy_exclusions_tab(
        &self,
        proxy: &ManagementProxy,
        snapshot: &UpstreamManagementSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_proxy_scope_tab(proxy, snapshot, false, cx)
    }

    fn render_proxy_scope_tab(
        &self,
        proxy: &ManagementProxy,
        snapshot: &UpstreamManagementSnapshot,
        assignments: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editor = &self.server_management.proxy_scope_editor;
        let draft = editor.draft.as_ref().filter(|draft| {
            draft.proxy_id == proxy.id && draft.selection.is_assignments() == assignments
        });
        let live = ScopeSelection::from_proxy(proxy, assignments);
        let selection = draft.map(|draft| &draft.selection).unwrap_or(&live);
        let base = draft.map(|draft| &draft.base).unwrap_or(&live);
        let editing = draft.is_some();
        let conflict = draft.is_some_and(|draft| draft.conflict);
        let busy = self.server_management.status.busy() || editor.saving;
        let can_assign = snapshot.has_permission(PROXIES_ASSIGN);
        let disabled = busy || !can_assign || conflict;
        let rows = selection_rows(proxy, snapshot, selection, base, editing);
        let edit_proxy_id = proxy.id.clone();
        let edit_button = Button::new("proxy-scope-edit")
            .label(if assignments {
                "Edit assignments"
            } else if proxy.excluded_user_ids.is_empty() && proxy.excluded_role_ids.is_empty() {
                "Add exclusion"
            } else {
                "Edit exclusions"
            })
            .small()
            .outline()
            .disabled(busy || !can_assign || editor.is_editing())
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.server_management.status.busy()
                    || this.server_management.proxy_scope_editor.is_editing()
                {
                    return;
                }
                let Some(snapshot) = this.server_management.snapshot.as_ref() else {
                    return;
                };
                if !snapshot.has_permission(PROXIES_ASSIGN) {
                    return;
                }
                if let Some(proxy) = snapshot
                    .proxies
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .find(|proxy| proxy.id == edit_proxy_id)
                {
                    this.server_management
                        .proxy_scope_editor
                        .begin(proxy, assignments);
                    cx.notify();
                }
            }));
        let mut content = v_flex().id("proxy-scope-scroll")
            .debug_selector(|| "proxy-scope-scroll".to_owned())
            .flex_1().min_h_0().overflow_y_scroll().w_full().gap_3().p_5()
            .child(h_flex().w_full().justify_between().gap_3()
                .child(div().text_lg().font_semibold().child(if assignments {
                    "Where this proxy applies"
                } else { "Who skips this proxy" }))
                .when(!editing, |row| row.child(
                    div().debug_selector(|| "proxy-scope-edit".to_owned()).child(edit_button),
                )))
            .child(scope_note(if assignments {
                "More specific rules take precedence per hostname: saved request → collection → workspace → server. Removing an assignment restores inheritance; it does not force direct routing."
            } else {
                "Excluded users and roles skip this proxy and fall through to the next applicable scope. Exclusions change routing, not permissions; they do not block requests or force direct routing."
            }, cx));
        if !can_assign {
            content = content.child(scope_note(if editing {
                "Editing permission is unavailable (proxies.assign). Your draft is preserved, but cannot be saved. Cancel to discard it."
            } else {
                "Read-only. Editing requires proxies.assign."
            }, cx));
        }
        if assignments && snapshot.workspaces.is_none() {
            content = content.child(scope_note("Workspace scopes are unavailable. Existing assignments are preserved; inaccessible entries can still be removed by ID.", cx));
        }
        if !assignments {
            if snapshot.users.is_none() {
                content = content.child(scope_note("Users are unavailable. Existing user exclusions are preserved and shown by ID.", cx));
            }
            if snapshot.roles.is_none() {
                content = content.child(scope_note("Roles are unavailable. Existing role exclusions are preserved and shown by ID.", cx));
            }
        }
        if conflict {
            content = content.child(scope_note("This list or one of its scope owners changed on the server. Your draft is preserved. Cancel and reopen to review the latest state before saving.", cx));
        }
        if let Some(error) = editor.save_error.as_ref() {
            content = content.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone()),
            );
        }
        if rows.is_empty() {
            content = content.child(scope_note(if assignments {
                "No assignments. This proxy does not apply to any scope."
            } else if editing {
                "No users or roles are available to select."
            } else {
                "No exclusions. Everyone in the assigned scopes uses these rules unless a more specific rule takes precedence."
            }, cx));
        }
        for (index, row) in rows.into_iter().enumerate() {
            let checked = selection.contains(&row.item);
            let occupied = row.owner.is_some();
            let label = match row.owner {
                Some(owner) => format!("{} · Assigned to {owner}", row.label),
                None => row.label,
            };
            let item = row.item;
            let row_selector = match &item {
                ScopeItem::Assignment(value) => format!(
                    "proxy-scope-row-assignment-{}-{}",
                    scope_kind_label(value.scope_kind)
                        .replace(' ', "-")
                        .to_lowercase(),
                    value.scope_id.as_deref().unwrap_or("server"),
                ),
                ScopeItem::User(id) => format!("proxy-scope-row-user-{id}"),
                ScopeItem::Role(id) => format!("proxy-scope-row-role-{id}"),
            };
            let row_proxy_id = proxy.id.clone();
            content = content.child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .py_2()
                    .debug_selector(move || row_selector.clone())
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .child(div().flex_1().min_w_0().text_sm().child(label))
                    .when(editing, |row| {
                        row.child(
                            Switch::new(SharedString::from(format!("proxy-scope-item-{index}")))
                                .checked(checked)
                                .disabled(disabled || occupied)
                                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                    if this.server_management.status.busy() {
                                        return;
                                    }
                                    let Some(snapshot) = this.server_management.snapshot.as_ref()
                                    else {
                                        return;
                                    };
                                    if !snapshot.has_permission(PROXIES_ASSIGN) {
                                        return;
                                    }
                                    let editor = &mut this.server_management.proxy_scope_editor;
                                    editor.reconcile(snapshot);
                                    if editor.saving {
                                        return;
                                    }
                                    if let Some(draft) = editor.draft.as_mut().filter(|draft| {
                                        draft.proxy_id == row_proxy_id && !draft.conflict
                                    }) {
                                        let occupied = match &item {
                                            ScopeItem::Assignment(value) => {
                                                assignment_owner(snapshot, &row_proxy_id, value)
                                                    .is_some()
                                            }
                                            _ => false,
                                        };
                                        if !occupied {
                                            draft.selection.set(&item, *checked);
                                        }
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            );
        }
        let mut body = v_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .debug_selector(move || {
                if assignments {
                    "proxy-assignments-tab".to_owned()
                } else {
                    "proxy-exclusions-tab".to_owned()
                }
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .debug_selector(move || {
                        if editing {
                            "proxy-scope-editor".to_owned()
                        } else {
                            "proxy-scope-readonly".to_owned()
                        }
                    })
                    .child(content),
            );
        if editing {
            let save_proxy_id = proxy.id.clone();
            body = body.child(
                h_flex()
                    .flex_shrink_0()
                    .gap_2()
                    .p_5()
                    .border_t_1()
                    .border_color(cx.api_outline_variant())
                    .debug_selector(|| "proxy-scope-footer".to_owned())
                    .child(
                        div()
                            .debug_selector(|| "proxy-scope-save".to_owned())
                            .child(
                                Button::new("proxy-scope-save")
                                    .label("Save")
                                    .small()
                                    .primary()
                                    .disabled(disabled)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if this.server_management.status.busy() {
                                            return;
                                        }
                                        let Some(snapshot) =
                                            this.server_management.snapshot.as_ref()
                                        else {
                                            return;
                                        };
                                        if !snapshot.has_permission(PROXIES_ASSIGN) {
                                            return;
                                        }
                                        let editor = &mut this.server_management.proxy_scope_editor;
                                        editor.reconcile(snapshot);
                                        if editor.saving {
                                            return;
                                        }
                                        let Some(draft) = editor.draft.as_ref() else {
                                            return;
                                        };
                                        if draft.proxy_id != save_proxy_id
                                            || draft.selection.is_assignments() != assignments
                                        {
                                            return;
                                        }
                                        if draft.conflict {
                                            cx.notify();
                                            return;
                                        }
                                        let mutation = match &draft.selection {
                                            ScopeSelection::Assignments(values) => {
                                                ManagementMutation::ReplaceProxyAssignments {
                                                    proxy_id: draft.proxy_id.clone(),
                                                    assignments: values.clone(),
                                                }
                                            }
                                            ScopeSelection::Exclusions { users, roles } => {
                                                ManagementMutation::ReplaceProxyExclusions {
                                                    proxy_id: draft.proxy_id.clone(),
                                                    excluded_user_ids: users.clone(),
                                                    excluded_role_ids: roles.clone(),
                                                }
                                            }
                                        };
                                        editor.saving = true;
                                        editor.save_error = None;
                                        this.run_management_mutation(mutation, window, cx);
                                    })),
                            ),
                    )
                    .child(self.render_proxy_scope_cancel(cx)),
            );
        }
        body.into_any_element()
    }
}

fn scope_note(text: &'static str, cx: &App) -> AnyElement {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assignment(kind: ProxyScopeKind, id: Option<&str>) -> ProxyAssignment {
        ProxyAssignment {
            scope_kind: kind,
            scope_id: id.map(str::to_owned),
        }
    }

    fn proxy() -> ManagementProxy {
        ManagementProxy {
            id: "proxy".into(),
            name: "Primary".into(),
            rules: vec![],
            assignments: vec![assignment(ProxyScopeKind::Workspace, Some("hidden"))],
            excluded_user_ids: vec!["unknown-user".into()],
            excluded_role_ids: vec!["unknown-role".into()],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn snapshot(proxy: ManagementProxy) -> UpstreamManagementSnapshot {
        UpstreamManagementSnapshot {
            current_user: crate::core::ManagementUser {
                id: "me".into(),
                email: "me@example.test".into(),
                display_name: "Me".into(),
                active: true,
                roles: vec![],
                created_by: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
            profiles: vec![],
            users: None,
            roles: None,
            permissions: None,
            workspaces: None,
            request_execution_settings: None,
            proxies: Some(vec![proxy]),
        }
    }

    #[test]
    fn unknown_items_survive_edits_and_remain_available_to_undo() {
        let proxy = proxy();
        let snapshot = snapshot(proxy.clone());
        let base = ScopeSelection::from_proxy(&proxy, true);
        let mut selection = base.clone();
        let server = ScopeItem::Assignment(assignment(ProxyScopeKind::Server, None));
        selection.set(&server, true);
        assert!(selection.contains(&ScopeItem::Assignment(proxy.assignments[0].clone())));
        selection.set(&ScopeItem::Assignment(proxy.assignments[0].clone()), false);
        let rows = selection_rows(&proxy, &snapshot, &selection, &base, true);
        assert!(
            rows.iter()
                .any(|row| row.label == "Inaccessible Workspace · ID: hidden")
        );
        let base = ScopeSelection::from_proxy(&proxy, false);
        let mut selection = base.clone();
        selection.set(&ScopeItem::User("visible".into()), true);
        assert!(selection.contains(&ScopeItem::User("unknown-user".into())));
        assert!(selection.contains(&ScopeItem::Role("unknown-role".into())));
        assert_eq!(
            selection_rows(&proxy, &snapshot, &selection, &base, false).len(),
            3
        );
    }

    #[test]
    fn stale_lists_block_save_but_unrelated_changes_do_not() {
        let proxy = proxy();
        let mut editor = ProxyScopeEditorState::default();
        editor.begin(&proxy, true);
        let mut snapshot = snapshot(proxy);
        snapshot.proxies.as_mut().unwrap()[0]
            .excluded_user_ids
            .clear();
        editor.reconcile(&snapshot);
        assert!(!editor.draft.as_ref().unwrap().conflict);
        snapshot.proxies.as_mut().unwrap()[0].assignments.clear();
        editor.reconcile(&snapshot);
        assert!(editor.draft.as_ref().unwrap().conflict);
        assert_eq!(
            editor.draft.as_ref().unwrap().selection,
            editor.draft.as_ref().unwrap().base
        );
    }

    #[test]
    fn exclusion_conflicts_compare_membership_not_order() {
        let mut proxy = proxy();
        proxy.excluded_user_ids.push("second-user".into());
        let mut editor = ProxyScopeEditorState::default();
        editor.begin(&proxy, false);
        let mut snapshot = snapshot(proxy);
        snapshot.proxies.as_mut().unwrap()[0]
            .excluded_user_ids
            .reverse();
        editor.reconcile(&snapshot);
        assert!(!editor.draft.as_ref().unwrap().conflict);
        snapshot.proxies.as_mut().unwrap()[0]
            .excluded_role_ids
            .clear();
        editor.reconcile(&snapshot);
        assert!(editor.draft.as_ref().unwrap().conflict);
        assert!(
            editor
                .draft
                .as_ref()
                .unwrap()
                .selection
                .contains(&ScopeItem::Role("unknown-role".into()))
        );
    }

    #[test]
    fn occupied_scopes_and_deleted_proxy_are_conflicts() {
        let proxy = proxy();
        let mut editor = ProxyScopeEditorState::default();
        editor.begin(&proxy, true);
        let mut snapshot = snapshot(proxy.clone());
        let mut other = proxy;
        other.id = "other".into();
        other.name = "Other proxy".into();
        snapshot.proxies.as_mut().unwrap().push(other);
        let draft = editor.draft.as_ref().unwrap();
        let rows = selection_rows(
            &snapshot.proxies.as_ref().unwrap()[0],
            &snapshot,
            &draft.selection,
            &draft.base,
            true,
        );
        assert!(
            rows.iter()
                .any(|row| row.owner.as_deref() == Some("Other proxy"))
        );
        editor.reconcile(&snapshot);
        assert!(editor.draft.as_ref().unwrap().conflict);
        editor.begin(&snapshot.proxies.as_ref().unwrap()[0], false);
        snapshot.proxies = None;
        editor.reconcile(&snapshot);
        assert!(editor.draft.as_ref().unwrap().conflict);
    }

    #[test]
    fn unavailable_proxy_preserves_original_identity_and_unsaved_selection() {
        for missing_list in [false, true] {
            let proxy = proxy();
            let mut editor = ProxyScopeEditorState::default();
            editor.begin(&proxy, false);
            let draft = editor.draft.as_mut().unwrap();
            draft
                .selection
                .set(&ScopeItem::User("draft-user".into()), true);
            let expected = draft.selection.clone();
            let mut snapshot = snapshot(proxy.clone());
            snapshot.proxies = if missing_list { None } else { Some(vec![]) };
            editor.reconcile(&snapshot);
            let draft = editor.draft.as_ref().unwrap();
            assert_eq!(draft.proxy_id, proxy.id);
            assert_eq!(draft.proxy_name, proxy.name);
            assert_eq!(draft.selection, expected);
            assert!(draft.conflict);
            snapshot.proxies = Some(vec![proxy]);
            editor.reconcile(&snapshot);
            assert!(editor.draft.as_ref().unwrap().conflict);
            editor.clear();
            assert!(!editor.is_editing());
        }
    }

    #[test]
    fn failed_save_preserves_draft_and_unrelated_success_does_not_close_it() {
        let mut editor = ProxyScopeEditorState::default();
        editor.begin(&proxy(), false);
        editor.finish_save(true);
        assert!(editor.is_editing());
        editor.saving = true;
        editor.finish_save(false);
        assert!(editor.is_editing());
        editor.saving = true;
        editor.finish_save(true);
        assert!(!editor.is_editing());
    }

    #[test]
    fn scope_paths_include_nested_collections_and_saved_requests() {
        let workspace: UpstreamWorkspaceView = serde_json::from_value(serde_json::json!({
            "id": "w", "name": "Workspace", "user_ids": [],
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "collections": [{
                "id": "c", "workspace_id": "w", "name": "Parent", "user_ids": [],
                "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
                "sub_collections": [{
                    "id": "nested", "workspace_id": "w", "name": "Child", "user_ids": [],
                    "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
                    "sub_collections": [], "requests": []
                }], "requests": []
            }]
        }))
        .unwrap();
        let mut workspace = workspace;
        workspace.collections[0].sub_collections[0]
            .requests
            .push(UpstreamSavedRequestView {
                id: "r".into(),
                collection_id: "nested".into(),
                name: "Fetch".into(),
                definition: Default::default(),
                created_by: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            });
        let workspaces = [workspace];
        let options = scope_options(Some(&workspaces));
        assert_eq!(options.len(), 5);
        assert_eq!(
            options[4].label,
            "Saved request · Workspace / Parent / Child / Fetch"
        );
        assert_eq!(
            proxy_assignment_label(&assignment(ProxyScopeKind::Server, None), None),
            "Server-wide · All workspaces"
        );
        assert_eq!(
            proxy_assignment_label(
                &assignment(ProxyScopeKind::Request, Some("r")),
                Some(&workspaces)
            ),
            options[4].label
        );
    }
}
