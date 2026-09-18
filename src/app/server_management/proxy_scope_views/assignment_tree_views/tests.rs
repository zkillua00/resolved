use super::*;
use crate::core::ManagementPermission;
use gpui::{Modifiers, TestAppContext, px, size};

fn snapshot() -> UpstreamManagementSnapshot {
    let now = Utc::now();
    let mut workspace: UpstreamWorkspaceView = serde_json::from_value(serde_json::json!({
        "id": "w", "name": "Workspace", "user_ids": ["actor"],
        "created_at": now, "updated_at": now,
        "collections": [{
            "id": "folder", "workspace_id": "w", "name": "API", "user_ids": [],
            "created_at": now, "updated_at": now, "requests": [],
            "sub_collections": [{
                "id": "nested", "workspace_id": "w", "name": "Direct access", "user_ids": [],
                "created_at": now, "updated_at": now, "sub_collections": [], "requests": []
            }]
        }, {
            "id": "sibling", "workspace_id": "w", "name": "Other APIs", "user_ids": [],
            "created_at": now, "updated_at": now, "sub_collections": [], "requests": []
        }]
    }))
    .unwrap();
    for id in ["a", "b"] {
        workspace.collections[0].sub_collections[0]
            .requests
            .push(UpstreamSavedRequestView {
                id: id.into(),
                collection_id: "nested".into(),
                name: format!("Request {id}"),
                definition: Default::default(),
                created_by: None,
                created_at: now,
                updated_at: now,
            });
    }
    UpstreamManagementSnapshot {
        current_user: ManagementUser {
            id: "actor".into(),
            email: "actor@example.test".into(),
            display_name: "Actor".into(),
            active: true,
            created_by: None,
            created_at: now,
            updated_at: now,
            roles: vec![ManagementRole {
                id: "editor".into(),
                name: "Editor".into(),
                description: String::new(),
                system: false,
                permissions: vec![ManagementPermission {
                    key: PROXIES_ASSIGN.into(),
                    description: String::new(),
                }],
                created_by: None,
                created_at: now,
                updated_at: now,
            }],
        },
        profiles: vec![],
        users: None,
        roles: None,
        permissions: None,
        workspaces: Some(vec![workspace]),
        request_execution_settings: None,
        proxies: Some(vec![ManagementProxy {
            id: "proxy".into(),
            name: "Routing".into(),
            rules: vec![],
            assignments: vec![],
            excluded_user_ids: vec![],
            excluded_role_ids: vec![],
            created_at: now,
            updated_at: now,
        }]),
    }
}

#[gpui::test]
fn native_tree_cascades_without_changing_the_live_snapshot(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let store = DatabaseStore::new(directory.path().join("test.sqlite3"));
    store.initialize().unwrap();
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
    cx.simulate_resize(size(px(1180.), px(820.)));
    cx.update(|window, cx| {
        window.activate_window();
        app.update(cx, |app, cx| {
            let now = Utc::now();
            app.settings.upstreams.upsert(UpstreamProfile {
                id: "server".into(),
                base_url: "https://server.example.test/".into(),
                user_id: "actor".into(),
                email: "actor@example.test".into(),
                display_name: "Actor".into(),
                session_expires_at: now + chrono::Duration::hours(1),
                connected_at: now,
                permission_keys: BTreeSet::from([PROXIES_ASSIGN.into()]),
                workspaces: vec![],
                active_workspace_id: None,
                active_environment_ids: BTreeMap::new(),
                extra: BTreeMap::new(),
            });
            app.settings.upstreams.select("server");
            app.server_management.upstream_id = Some("server".into());
            app.server_management.status = ServerManagementStatus::Ready;
            app.server_management.set_snapshot(snapshot());
            app.ensure_proxy_workspace_inputs(window, cx);
            app.workspace_tabs.open_tool(WorkspaceToolTab::RequestProxy);
            cx.notify();
        });
    });
    cx.run_until_parked();
    for selector in [
        "proxy-tab-assignments",
        "proxy-scope-edit",
        "proxy-assignment-toggle-workspace-w",
        "proxy-assignment-disclosure-collection-folder",
        "proxy-assignment-disclosure-collection-nested",
    ] {
        let bounds = cx.debug_bounds(selector).expect(selector);
        cx.simulate_click(bounds.center(), Modifiers::none());
        cx.run_until_parked();
    }
    let workspace = cx.debug_bounds("proxy-assignment-row-workspace-w").unwrap();
    let folder = cx
        .debug_bounds("proxy-assignment-row-collection-folder")
        .unwrap();
    let nested = cx
        .debug_bounds("proxy-assignment-row-collection-nested")
        .unwrap();
    let request = cx.debug_bounds("proxy-assignment-row-request-a").unwrap();
    assert!(workspace.origin.x < folder.origin.x);
    assert!(folder.origin.x < nested.origin.x);
    assert!(nested.origin.x < request.origin.x);
    let footer = cx.debug_bounds("proxy-scope-footer").unwrap();
    let page = cx.debug_bounds("request-proxy-workspace").unwrap();
    assert!(footer.origin.y + footer.size.height <= page.origin.y + page.size.height + px(1.));

    let child = cx
        .debug_bounds("proxy-assignment-toggle-request-a")
        .unwrap();
    cx.simulate_click(child.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let app = app.read(cx);
        let snapshot = app.server_management.snapshot.as_ref().unwrap();
        let draft = app
            .server_management
            .proxy_scope_editor
            .draft
            .as_ref()
            .unwrap();
        let ScopeSelection::Assignments(selected) = &draft.selection else {
            panic!("assignment draft");
        };
        let tree = AssignmentTree::new(snapshot, "proxy", selected, &[]);
        for (kind, id, expected) in [
            (ProxyScopeKind::Workspace, "w", AssignmentCheckState::Mixed),
            (
                ProxyScopeKind::Collection,
                "folder",
                AssignmentCheckState::Mixed,
            ),
            (
                ProxyScopeKind::Collection,
                "nested",
                AssignmentCheckState::Mixed,
            ),
            (
                ProxyScopeKind::Collection,
                "sibling",
                AssignmentCheckState::Checked,
            ),
            (
                ProxyScopeKind::Request,
                "a",
                AssignmentCheckState::Unchecked,
            ),
            (ProxyScopeKind::Request, "b", AssignmentCheckState::Checked),
        ] {
            assert_eq!(
                tree.check_state(
                    &ProxyAssignment {
                        scope_kind: kind,
                        scope_id: Some(id.into()),
                    },
                    selected
                ),
                expected
            );
        }
        assert!(snapshot.proxies.as_ref().unwrap()[0].assignments.is_empty());
        assert!(!app.server_management.status.busy());
    });
    let collapse = cx
        .debug_bounds("proxy-assignment-disclosure-collection-folder")
        .unwrap();
    cx.simulate_click(collapse.center(), Modifiers::none());
    cx.run_until_parked();
    let cancel = cx.debug_bounds("proxy-scope-cancel").unwrap();
    cx.simulate_click(cancel.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let editor = &app.read(cx).server_management.proxy_scope_editor;
        assert!(!editor.is_editing());
        assert_eq!(editor.tree_expansion.get("collection-folder"), Some(&false));
    });
}
