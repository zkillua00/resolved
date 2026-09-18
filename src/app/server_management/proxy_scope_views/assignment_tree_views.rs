use super::assignment_tree::{
    AssignmentCheckState, AssignmentNode, AssignmentTree, assignment_key,
};
use super::*;
use gpui_component::Icon;

impl ApiTester {
    pub(super) fn render_proxy_assignment_tree(
        &self,
        proxy: &ManagementProxy,
        snapshot: &UpstreamManagementSnapshot,
        selected: &[ProxyAssignment],
        baseline: &[ProxyAssignment],
        editing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tree = AssignmentTree::new(snapshot, &proxy.id, selected, baseline);
        let mut branch_keys = Vec::new();
        collect_branch_keys(&tree.roots, &mut branch_keys);
        let expand_keys = branch_keys.clone();
        let collapse_keys = branch_keys;
        let rows = tree
            .roots
            .iter()
            .filter(|node| {
                editing
                    || tree.check_state(&node.assignment, selected)
                        != AssignmentCheckState::Unchecked
            })
            .map(|node| {
                self.render_assignment_branch(&tree, node, &proxy.id, selected, editing, cx)
            })
            .collect::<Vec<_>>();

        v_flex()
            .debug_selector(|| "proxy-assignment-tree".to_owned())
            .w_full()
            .gap_2()
            .child(
                h_flex().justify_between().gap_3().py_2()
                    .child(div().text_xs().text_color(cx.theme().muted_foreground)
                        .child(format!("{} explicit scope assignments", selected.len())))
                    .child(h_flex().gap_1()
                        .child(Button::new("proxy-scopes-expand-all").label("Expand all").xsmall().ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                for key in &expand_keys {
                                    this.server_management.proxy_scope_editor.tree_expansion.insert(key.clone(), true);
                                }
                                cx.notify();
                            })))
                        .child(Button::new("proxy-scopes-collapse-all").label("Collapse all").xsmall().ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                for key in &collapse_keys {
                                    this.server_management.proxy_scope_editor.tree_expansion.insert(key.clone(), false);
                                }
                                cx.notify();
                            })))),
            )
            .when(selected.iter().any(|value| value.scope_kind == ProxyScopeKind::Server), |view| {
                view.child(div().py_2().text_xs().text_color(cx.theme().muted_foreground)
                    .child("Server-wide includes every workspace. Clear it before making folder-level exceptions; hidden workspaces cannot be safely split from this view."))
            })
            .when(rows.is_empty(), |view| {
                view.child(div().py_5().text_sm().text_color(cx.theme().muted_foreground)
                    .child("No assignments. Edit assignments to select workspaces or folders."))
            })
            .children(rows)
            .when(editing, |view| {
                view.child(div().pt_3().text_xs().text_color(cx.theme().muted_foreground)
                    .child("Partial selections replace parent coverage with the remaining visible branches, including removing workspace-level ad-hoc coverage. Other proxies may still apply to unchecked items."))
            })
            .into_any_element()
    }

    fn render_assignment_branch(
        &self,
        tree: &AssignmentTree,
        node: &AssignmentNode,
        proxy_id: &str,
        selected: &[ProxyAssignment],
        editing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = tree.check_state(&node.assignment, selected);
        let key = assignment_key(&node.assignment);
        let expanded = self
            .server_management
            .proxy_scope_editor
            .tree_expansion
            .get(&key)
            .copied()
            .unwrap_or(
                matches!(
                    node.assignment.scope_kind,
                    ProxyScopeKind::Server | ProxyScopeKind::Workspace
                ) || state == AssignmentCheckState::Mixed,
            );
        let branch = !node.children.is_empty();
        let editor = &self.server_management.proxy_scope_editor;
        let conflict = editor.draft.as_ref().is_some_and(|draft| draft.conflict);
        let can_assign = self
            .server_management
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.has_permission(PROXIES_ASSIGN));
        let disabled = !editing
            || !can_assign
            || conflict
            || editor.saving
            || self.server_management.status.busy()
            || node.owner.is_some();
        let checked = state == AssignmentCheckState::Checked;
        let mixed = state == AssignmentCheckState::Mixed;
        let direct = selected.contains(&node.assignment);
        let row_selector = format!("proxy-assignment-row-{key}");
        let toggle_selector = format!("proxy-assignment-toggle-{key}");
        let disclosure_selector = format!("proxy-assignment-disclosure-{key}");
        let disclosure_key = key.clone();
        let toggle_proxy_id = proxy_id.to_owned();
        let assignment = node.assignment.clone();
        let path = node.path.clone();
        let state_label = match state {
            AssignmentCheckState::Checked if direct => "Assigned",
            AssignmentCheckState::Checked => "Inherited",
            AssignmentCheckState::Mixed => "Selected branches",
            AssignmentCheckState::Unchecked => "Not assigned",
        };
        let tooltip = if let Some(owner) = &node.owner {
            format!("This scope belongs to {owner}. Its rules may layer with inherited rules.")
        } else {
            format!(
                "{} — {state_label}. {}",
                node.label,
                if mixed {
                    "Check to include the entire folder, including future children."
                } else if checked {
                    "Uncheck this branch."
                } else {
                    "Check this branch and its descendants."
                }
            )
        };
        // The upstream Checkbox only supports two states. Use a native,
        // keyboard-focusable square button for the explicit mixed state.
        let checkbox = Button::new(SharedString::from(format!("assignment-check-{key}")))
            .xsmall()
            .outline()
            .w(px(20.))
            .h(px(20.))
            .p_0()
            .rounded_sm()
            .when(checked, |button| button.icon(IconName::Check).primary())
            .when(mixed, |button| button.icon(IconName::Minus))
            .disabled(disabled)
            .tooltip(tooltip)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_proxy_assignment_branch(&toggle_proxy_id, &assignment, !checked, cx);
            }));
        let row = h_flex()
            .id(SharedString::from(format!("assignment-row-{key}")))
            .debug_selector(move || row_selector.clone())
            .w_full()
            .h(px(36.))
            .flex_shrink_0()
            .gap_2()
            .px_1()
            .rounded_sm()
            .hover(|style| style.bg(cx.api_surface_high()))
            .child(if branch {
                div()
                    .debug_selector(move || disclosure_selector.clone())
                    .child(
                        Button::new(SharedString::from(format!("assignment-disclosure-{key}")))
                            .icon(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .xsmall()
                            .ghost()
                            .w(px(22.))
                            .h(px(24.))
                            .tooltip(format!(
                                "{} {}",
                                if expanded { "Collapse" } else { "Expand" },
                                node.label
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.server_management
                                    .proxy_scope_editor
                                    .tree_expansion
                                    .insert(disclosure_key.clone(), !expanded);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            } else {
                div().w(px(22.)).flex_shrink_0().into_any_element()
            })
            .child(
                div()
                    .debug_selector(move || toggle_selector.clone())
                    .child(checkbox),
            )
            .child(
                Icon::new(match node.assignment.scope_kind {
                    ProxyScopeKind::Server | ProxyScopeKind::Workspace => IconName::Globe,
                    ProxyScopeKind::Collection if expanded && branch => IconName::FolderOpen,
                    ProxyScopeKind::Collection => IconName::FolderClosed,
                    ProxyScopeKind::Request => IconName::File,
                })
                .with_size(px(15.))
                .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .id(SharedString::from(format!("assignment-name-{key}")))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .when(
                        node.assignment.scope_kind != ProxyScopeKind::Request,
                        |label| label.font_medium(),
                    )
                    .tooltip(move |window, cx| Tooltip::new(path.clone()).build(window, cx))
                    .child(node.label.clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .text_color(cx.theme().muted_foreground)
                    .child(if let Some(owner) = &node.owner {
                        format!("Assigned to {owner}")
                    } else if !node.available {
                        "Unavailable scope".to_owned()
                    } else if mixed || checked {
                        state_label.to_owned()
                    } else if branch {
                        format!("{} items", node.children.len())
                    } else {
                        String::new()
                    }),
            );
        let children = if expanded {
            node.children
                .iter()
                .filter(|child| {
                    editing
                        || tree.check_state(&child.assignment, selected)
                            != AssignmentCheckState::Unchecked
                })
                .map(|child| {
                    self.render_assignment_branch(tree, child, proxy_id, selected, editing, cx)
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        v_flex()
            .w_full()
            .flex_shrink_0()
            .child(row)
            .when(!children.is_empty(), |view| {
                view.child(
                    v_flex()
                        .ml(px(15.))
                        .pl(px(12.))
                        .border_l_1()
                        .border_color(cx.api_outline_variant())
                        .children(children),
                )
            })
            .into_any_element()
    }

    fn toggle_proxy_assignment_branch(
        &mut self,
        proxy_id: &str,
        assignment: &ProxyAssignment,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        if self.server_management.status.busy() {
            return;
        }
        let Some(snapshot) = self.server_management.snapshot.as_ref() else {
            return;
        };
        if !snapshot.has_permission(PROXIES_ASSIGN) {
            return;
        }
        let editor = &mut self.server_management.proxy_scope_editor;
        editor.reconcile(snapshot);
        if editor.saving {
            return;
        }
        let Some(draft) = editor
            .draft
            .as_mut()
            .filter(|draft| draft.proxy_id == proxy_id && !draft.conflict)
        else {
            return;
        };
        let (ScopeSelection::Assignments(selected), ScopeSelection::Assignments(baseline)) =
            (&draft.selection, &draft.base)
        else {
            return;
        };
        let tree = AssignmentTree::new(snapshot, proxy_id, selected, baseline);
        match tree.toggle(selected, assignment, checked) {
            Ok(assignments) => {
                draft.selection = ScopeSelection::Assignments(assignments);
                editor.save_error = None;
            }
            Err(error) => editor.save_error = Some(error),
        }
        cx.notify();
    }
}

fn collect_branch_keys(nodes: &[AssignmentNode], keys: &mut Vec<String>) {
    for node in nodes {
        if !node.children.is_empty() {
            keys.push(assignment_key(&node.assignment));
            collect_branch_keys(&node.children, keys);
        }
    }
}

#[cfg(test)]
mod tests;
