use super::*;
use std::time::{Duration, Instant};

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
