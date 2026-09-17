use super::*;
use crate::core::{ManagementPermission, PROXIES_READ};
use gpui::{Modifiers, TestAppContext, px, size};

fn snapshot() -> UpstreamManagementSnapshot {
    let now = Utc::now();
    let permissions = [
        SERVER_SETTINGS_UPDATE,
        PROXIES_READ,
        PROXIES_CREATE,
        PROXIES_UPDATE,
        PROXIES_DELETE,
        PROXIES_ASSIGN,
    ]
    .into_iter()
    .map(|key| ManagementPermission {
        key: key.to_owned(),
        description: key.to_owned(),
    })
    .collect();
    UpstreamManagementSnapshot {
        current_user: ManagementUser {
            id: "user-1".into(),
            email: "owner@example.test".into(),
            display_name: "Owner".into(),
            active: true,
            roles: vec![ManagementRole {
                id: "owner".into(),
                name: "Owner".into(),
                description: "Server owner".into(),
                system: true,
                permissions,
                created_by: None,
                created_at: now,
                updated_at: now,
            }],
            created_by: None,
            created_at: now,
            updated_at: now,
        },
        profiles: vec![],
        users: Some(vec![]),
        roles: Some(vec![]),
        permissions: None,
        workspaces: Some(vec![]),
        request_execution_settings: Some(RequestExecutionSettings {
            mode: RequestExecutionMode::Server,
        }),
        proxies: Some(vec![ManagementProxy {
            id: "proxy-1".into(),
            name: "Server default".into(),
            rules: vec![
                HostnameOverride {
                    hostname: "api.internal".into(),
                    target: "10.0.0.25".into(),
                },
                HostnameOverride {
                    hostname: "legacy.internal".into(),
                    target: "https://gateway.internal".into(),
                },
            ],
            assignments: vec![ProxyAssignment {
                scope_kind: ProxyScopeKind::Server,
                scope_id: None,
            }],
            excluded_user_ids: vec![],
            excluded_role_ids: vec![],
            created_at: now,
            updated_at: now,
        }]),
    }
}

#[test]
fn selection_survives_refresh_and_reconciles_removed_rules_and_proxies() {
    let mut snapshot = snapshot();
    let mut state = RequestProxyState::default();
    state.reconcile(&snapshot);
    assert_eq!(state.selected_proxy_id.as_deref(), Some("proxy-1"));
    state.selected_rule_hostname = Some("legacy.internal".into());
    state.tab = ProxyTab::Exclusions;
    state.reconcile(&snapshot);
    assert_eq!(
        state.selected_rule_hostname.as_deref(),
        Some("legacy.internal")
    );
    assert_eq!(state.tab, ProxyTab::Exclusions);

    snapshot.proxies.as_mut().unwrap()[0].rules.pop();
    state.reconcile(&snapshot);
    assert_eq!(
        state.selected_rule_hostname.as_deref(),
        Some("api.internal")
    );
    snapshot.proxies = None;
    state.reconcile(&snapshot);
    assert!(state.selected_proxy_id.is_none());
    assert!(state.selected_rule_hostname.is_none());
}

#[test]
fn search_and_summaries_do_not_fabricate_scope_or_status() {
    assert!(filter_matches("  API  ", &["api.internal", "10.0.0.25"]));
    assert!(filter_matches("0.25", &["api.internal", "10.0.0.25"]));
    assert!(!filter_matches("missing", &["api.internal", "10.0.0.25"]));
    assert!(filter_matches(" ", &["api.internal"]));
    assert_eq!(count_label(1, "rule"), "1 rule");
    assert_eq!(count_label(3, "rule"), "3 rules");
    let mut proxy = snapshot().proxies.unwrap().remove(0);
    assert_eq!(proxy_scope_summary(&proxy), "Server-wide");
    proxy.assignments.clear();
    assert_eq!(proxy_scope_summary(&proxy), "Unassigned");
}

/// Exercise the actual native surface with an isolated database and a synthetic
/// management snapshot. Never load credentials or send a management mutation.
#[gpui::test]
fn request_proxy_renders_as_a_full_workspace_tool(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
    store.initialize().expect("initialize test database");
    let mut app = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        gpui_component::init(cx);
        let bindings = shortcuts::capture_base_key_bindings(cx);
        crate::theme::configure(cx);
        let view = cx.new(|cx| ApiTester::new_with_database_store(bindings, store, window, cx));
        crate::register_app_action_handlers(&view, cx);
        app = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });
    let app = app.unwrap();
    cx.update(|window, cx| {
        window.activate_window();
        app.update(cx, |app, cx| {
            let now = Utc::now();
            let snapshot = snapshot();
            app.settings.upstreams.upsert(UpstreamProfile {
                id: "server-1".into(),
                base_url: "https://resolved.example.test/".into(),
                user_id: snapshot.current_user.id.clone(),
                email: snapshot.current_user.email.clone(),
                display_name: snapshot.current_user.display_name.clone(),
                session_expires_at: now + chrono::Duration::hours(1),
                connected_at: now,
                permission_keys: BTreeSet::from([SERVER_SETTINGS_UPDATE.into()]),
                workspaces: vec![],
                active_workspace_id: None,
                active_environment_ids: BTreeMap::new(),
                extra: BTreeMap::new(),
            });
            assert!(app.settings.upstreams.select("server-1"));
            app.server_management.upstream_id = Some("server-1".into());
            app.server_management.status = ServerManagementStatus::Ready;
            app.server_management.set_snapshot(snapshot);
            app.ensure_proxy_workspace_inputs(window, cx);
            app.workspace_tabs.open_tool(WorkspaceToolTab::RequestProxy);
            cx.notify();
        });
    });

    for (width, height) in [(1440., 920.), (1180., 820.)] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.run_until_parked();
        for selector in [
            "request-proxy-workspace",
            "workspace-request-proxy-tab",
            "proxy-execution-location",
            "proxy-sidebar",
            "proxy-detail",
            "new-proxy",
            "proxy-list-proxy-1",
            "proxy-rule-list",
            "proxy-rule-proxy-1-api.internal",
            "proxy-rule-proxy-1-legacy.internal",
            "proxy-rule-inspector",
            "proxy-tab-rules",
            "proxy-tab-assignments",
            "proxy-tab-exclusions",
        ] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "{selector} at {width}×{height}"
            );
        }
        let workspace = cx.debug_bounds("request-proxy-workspace").unwrap();
        let sidebar = cx.debug_bounds("proxy-sidebar").unwrap();
        let list = cx.debug_bounds("proxy-rule-list").unwrap();
        let inspector = cx.debug_bounds("proxy-rule-inspector").unwrap();
        assert!(sidebar.origin.x + sidebar.size.width <= list.origin.x + px(1.));
        assert!(list.origin.x + list.size.width <= inspector.origin.x + px(1.));
        assert!(
            inspector.origin.x + inspector.size.width
                <= workspace.origin.x + workspace.size.width + px(1.)
        );
        assert!(list.size.width > px(250.));

        let edit = cx.debug_bounds("proxy-rule-edit-action").unwrap();
        cx.simulate_click(edit.center(), Modifiers::none());
        cx.run_until_parked();
        assert!(cx.debug_bounds("proxy-rule-editor").is_some());
        cx.update(|_, cx| {
            assert!(
                app.read(cx)
                    .server_management
                    .proxy_rule_editor
                    .is_editing()
            )
        });
        let footer = cx.debug_bounds("proxy-rule-editor-footer").unwrap();
        assert!(
            footer.origin.y + footer.size.height
                <= workspace.origin.y + workspace.size.height + px(1.)
        );
        let cancel = cx.debug_bounds("cancel-proxy-rule").unwrap();
        cx.simulate_click(cancel.center(), Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(
                !app.read(cx).server_management.proxy_rule_editor.is_editing(),
                "Cancel did not close the draft at {width}×{height}; workspace={workspace:?}, inspector={inspector:?}, footer={footer:?}, cancel={cancel:?}"
            );
        });
        // GPUI's debug-bounds cache retains removed selectors, so state is the
        // reliable assertion for removal; bounds above verify visible placement.
    }

    let row = cx
        .debug_bounds("proxy-rule-proxy-1-legacy.internal")
        .unwrap();
    cx.simulate_click(row.center(), Modifiers::none());
    cx.update(|_, cx| {
        assert_eq!(
            app.read(cx)
                .server_management
                .proxy_workspace
                .selected_rule_hostname
                .as_deref(),
            Some("legacy.internal")
        );
    });

    cx.update(|window, cx| {
        let input = app
            .read(cx)
            .server_management
            .proxy_workspace
            .rule_search
            .clone()
            .unwrap();
        input.update(cx, |input, cx| input.set_value("GATEWAY", window, cx));
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("proxy-rule-proxy-1-legacy.internal")
            .is_some()
    );

    for (tab, panel) in [
        ("proxy-tab-assignments", "proxy-assignments-tab"),
        ("proxy-tab-exclusions", "proxy-exclusions-tab"),
    ] {
        let bounds = cx.debug_bounds(tab).unwrap();
        cx.simulate_click(bounds.center(), Modifiers::none());
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(panel).is_some(),
            "{panel} must render when selected"
        );
        cx.update(|_, cx| {
            assert_ne!(
                app.read(cx).server_management.proxy_workspace.tab,
                ProxyTab::Rules
            );
        });
    }

    // A draft remains reachable after permission loss or removal of its proxy.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.server_management.proxy_workspace.tab = ProxyTab::Rules;
            let proxy = app.management_proxy("proxy-1").unwrap().clone();
            app.open_proxy_rule_editor(proxy.id, Some(proxy.rules[0].clone()), window, cx);
            let mut snapshot = snapshot();
            snapshot.proxies = None;
            snapshot.request_execution_settings = None;
            snapshot.current_user.roles.clear();
            app.server_management.set_snapshot(snapshot);
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("proxy-rule-editor").is_some());
    let cancel = cx.debug_bounds("cancel-proxy-rule").unwrap();
    cx.simulate_click(cancel.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(
            !app.read(cx)
                .server_management
                .proxy_rule_editor
                .is_editing()
        );
        assert_eq!(
            app.read(cx).server_management.status,
            ServerManagementStatus::Ready
        );
    });

    // Neither network failure nor session expiry may hide or discard an edit.
    // Refresh takes the expired-session early return, so this never reads a
    // credential or contacts even the synthetic upstream URL.
    for tab in [ProxyTab::Rules, ProxyTab::Assignments, ProxyTab::Exclusions] {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.server_management.status = ServerManagementStatus::Ready;
                app.server_management.set_snapshot(snapshot());
                app.server_management.proxy_workspace.tab = tab;
                if tab == ProxyTab::Rules {
                    let proxy = app.management_proxy("proxy-1").unwrap().clone();
                    app.open_proxy_rule_editor(proxy.id, Some(proxy.rules[0].clone()), window, cx);
                }
                cx.notify();
            });
        });
        cx.run_until_parked();
        if tab != ProxyTab::Rules {
            let edit = cx.debug_bounds("proxy-scope-edit").unwrap();
            cx.simulate_click(edit.center(), Modifiers::none());
            cx.run_until_parked();
        }
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                assert!(if tab == ProxyTab::Rules {
                    app.server_management.proxy_rule_editor.is_editing()
                } else {
                    app.server_management.proxy_scope_editor.is_editing()
                });
                app.server_management.status = ServerManagementStatus::Error("Offline".into());
                app.server_management.snapshot = None;
                cx.notify();
            });
        });
        cx.run_until_parked();
        let cancel_selector = if tab == ProxyTab::Rules {
            "cancel-proxy-rule"
        } else {
            "proxy-scope-cancel"
        };
        assert!(cx.debug_bounds(cancel_selector).is_some());
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.settings
                    .upstreams
                    .servers
                    .iter_mut()
                    .find(|profile| profile.id == "server-1")
                    .unwrap()
                    .session_expires_at = Utc::now() - chrono::Duration::minutes(1);
                app.refresh_server_management(window, cx);
                assert!(if tab == ProxyTab::Rules {
                    app.server_management.proxy_rule_editor.is_editing()
                } else {
                    app.server_management.proxy_scope_editor.is_editing()
                });
                assert!(matches!(
                    app.server_management.status,
                    ServerManagementStatus::Error(_)
                ));
            });
        });
        cx.run_until_parked();
        let cancel = cx.debug_bounds(cancel_selector).unwrap();
        cx.simulate_click(cancel.center(), Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(
                !app.read(cx)
                    .server_management
                    .proxy_rule_editor
                    .is_editing()
            );
            assert!(
                !app.read(cx)
                    .server_management
                    .proxy_scope_editor
                    .is_editing()
            );
        });
    }
}

#[test]
fn creation_selection_uses_returned_id_not_another_participants_new_proxy() {
    let mut snapshot = snapshot();
    let proxies = snapshot.proxies.as_mut().unwrap();
    let mut other = proxies[0].clone();
    other.id = "concurrently-created".into();
    let mut mine = proxies[0].clone();
    mine.id = "created-by-this-action".into();
    proxies.extend([other, mine]);
    let mut state = RequestProxyState::default();
    state.select_proxy_by_id("created-by-this-action", Some(proxies));
    state.reconcile(&snapshot);
    assert_eq!(
        state.selected_proxy_id.as_deref(),
        Some("created-by-this-action")
    );
}
