use super::*;
use std::time::{Duration, Instant};

#[gpui::test]
fn restoring_a_tab_refreshes_intelligence_only_once(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let store = DatabaseStore::new(directory.path().join("restore.sqlite3"));
    let (_, cx) = cx.add_window_view(|window, cx| {
        gpui_component::init(cx);
        crate::theme::configure(cx);
        let bindings = shortcuts::capture_base_key_bindings(cx);
        let view = cx.new(|cx| ApiTester::new_with_database_store(bindings, store, window, cx));
        let calls = Rc::new(std::cell::Cell::new(0));
        let counted = calls.clone();
        let editor = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default().diagnostic_provider(move |_| {
                    counted.set(counted.get() + 1);
                    Vec::new()
                }),
                window,
                cx,
            )
        });
        view.update(cx, |app, cx| {
            app.pre_request_script = editor;
            let mut template =
                RequestTemplate::new(RequestDraft::new("GET", "https://example.test/"));
            template.scripts.pre_request = "console.log('restored');".into();
            app.request_tabs.open_unsaved(
                "Restore target",
                template,
                RequestTabAssociation::default(),
            );
            calls.set(0);
            app.restore_active_request_tab(window, cx);
            // set_value refreshes once, and refreshing the workspace catalogs
            // refreshes once. A second full intelligence pass is redundant.
            assert_eq!(calls.get(), 2);
            assert_eq!(
                app.pre_request_script.read(cx).value(cx).as_ref(),
                "console.log('restored');"
            );
            assert_eq!(app.url.read(cx).value().as_ref(), "https://example.test/");
            assert!(!app.request_is_dirty());
        });
        gpui_component::Root::new(view, window, cx)
    });
    cx.run_until_parked();
}

/// An opt-in wall-clock diagnostic, not a timing assertion. GPUI's test context
/// draws invalidated windows while flushing an update, so the enclosing update
/// measures both effects and the CPU frame. Do not draw again manually.
/// This does not simulate macOS key repeat or GPU presentation.
#[gpui::test]
#[ignore = "manual no-response tab switching measurement"]
fn no_response_tab_switch_timings(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let store = DatabaseStore::new(directory.path().join("switching.sqlite3"));
    let mut app = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        gpui_component::init(cx);
        let bindings = shortcuts::capture_base_key_bindings(cx);
        crate::theme::configure(cx);
        let view = cx.new(|cx| ApiTester::new_with_database_store(bindings, store, window, cx));
        app = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });
    let app = app.unwrap();
    let tabs = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let request_count = std::env::var("RESOLVED_TAB_BENCH_REQUESTS")
                .ok()
                .map(|value| {
                    value
                        .parse::<usize>()
                        .expect("request count must be a number")
                })
                .unwrap_or(0);
            let mut workspace = Workspace::default();
            for collection_index in 0..request_count.div_ceil(100) {
                let collection_id = workspace
                    .create_collection(format!("Collection {collection_index}"))
                    .unwrap();
                for index in
                    collection_index * 100..((collection_index + 1) * 100).min(request_count)
                {
                    workspace
                        .create_saved_request(
                            &collection_id,
                            format!("Request {index}"),
                            RequestTemplate::new(RequestDraft::new(
                                "GET",
                                format!("https://example.test/saved/{index}"),
                            )),
                        )
                        .unwrap();
                }
            }
            app.replace_workspace(workspace);
            eprintln!("fixture: {request_count} saved requests, four response-less tabs");
            let tabs = (0..4)
                .map(|index| {
                    app.request_tabs.open_unsaved(
                        format!("Request {index}"),
                        RequestTemplate::new(RequestDraft::new(
                            "GET",
                            format!("https://example.test/{index}"),
                        )),
                        RequestTabAssociation::default(),
                    )
                })
                .collect::<Vec<_>>();
            app.workspace_tabs = WorkspaceTabs::from_request_tabs(&app.request_tabs);
            app.workspace_tabs.activate_request();
            app.sidebar_tab = SidebarTab::Collections;
            app.restore_active_request_tab(window, cx);
            tabs
        })
    });
    cx.run_until_parked();

    for (label, switching) in [("same tab redraw", false), ("switching tabs", true)] {
        let mut activation = Vec::new();
        let mut update = Vec::new();
        let mut tasks = Vec::new();
        for iteration in 0..104 {
            let index = if switching { iteration % tabs.len() } else { 0 };
            let start = Instant::now();
            let elapsed = cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    let start = Instant::now();
                    app.activate_request_tab(tabs[index].clone(), window, cx);
                    assert!(app.response.is_none());
                    start.elapsed()
                })
            });
            let update_elapsed = start.elapsed();
            let tasks_start = Instant::now();
            cx.run_until_parked();
            if iteration >= 4 {
                activation.push(elapsed);
                update.push(update_elapsed);
                tasks.push(tasks_start.elapsed());
            }
        }
        print_timings(&format!("{label}: activation callback"), activation);
        print_timings(&format!("{label}: update + CPU frame"), update);
        print_timings(&format!("{label}: queued tasks"), tasks);
    }
}

fn print_timings(label: &str, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    let count = samples.len();
    let mean = samples.iter().map(Duration::as_secs_f64).sum::<f64>() / count as f64;
    eprintln!(
        "{label}: mean={:.3}ms p50={:.3}ms p95={:.3}ms ({count} samples)",
        mean * 1000.,
        samples[count / 2].as_secs_f64() * 1000.,
        samples[count * 95 / 100].as_secs_f64() * 1000.,
    );
}
