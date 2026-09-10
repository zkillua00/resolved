//! Native regression probe for opening an empty Documentation editor.
//!
//! Build with `./scripts/cargo.sh build --example documentation_placeholder_probe`,
//! then run `target/debug/examples/documentation_placeholder_probe`.
//! Unlike GPUI's test platform, this uses the real platform text shaper. It opens
//! a temporary window, exercises empty/populated/empty editors, then exits.
//! No application database, credentials or network requests are involved.
use std::time::Duration;

use gpui::{
    App, AppContext as _, Application, Bounds, Context, Entity, IntoElement, ParentElement as _,
    Render, Styled as _, Window, WindowBounds, WindowOptions, div, px, size,
};
use gpui_component::{
    Root,
    input::{Input, InputState},
};

const DOCUMENTATION_PLACEHOLDER: &str = "Markdown notes\n\n@param query.limit Maximum records per page.\n@header Authorization Token obtained from Login.\n\nType @ at the start of a line to reference a field.";

struct Probe {
    editor: Entity<InputState>,
    show_documentation: bool,
    rendered: bool,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.rendered = self.show_documentation;
        let content = if self.show_documentation {
            Input::new(&self.editor).size_full().into_any_element()
        } else {
            div().child("Opening Documentation…").into_any_element()
        };
        div().size_full().child(content)
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);
        let bounds = Bounds::centered(None, size(px(800.), px(600.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    let editor = cx.new(|cx| {
                        InputState::new(window, cx)
                            .code_editor("markdown")
                            .rows(12)
                            .soft_wrap(true)
                            .placeholder(DOCUMENTATION_PLACEHOLDER)
                    });
                    let probe = cx.new(|_| Probe {
                        editor,
                        show_documentation: false,
                        rendered: false,
                    });
                    cx.new(|cx| Root::new(probe, window, cx))
                },
            )
            .expect("open native probe window");
        cx.activate(true);
        cx.spawn(async move |cx| {
            // Let native layout and painting finish between state transitions.
            gpui::Timer::after(Duration::from_millis(300)).await;
            for (placeholder, value) in [
                (DOCUMENTATION_PLACEHOLDER, ""),
                (DOCUMENTATION_PLACEHOLDER, "@header Authorization Token."),
                (DOCUMENTATION_PLACEHOLDER, ""),
                ("\nRésumé 🔎\n\n@param query.标签 Unicode.\n", ""),
                ("\r\nRésumé 🔎\r\n\r\n@param query.标签 Unicode.\r\n", ""),
                ("", ""),
                ("Single line", ""),
            ] {
                window
                    .update(cx, |root, window, cx| {
                        let view = root.view().clone().downcast::<Probe>().expect("probe root");
                        view.update(cx, |probe, cx| {
                            probe.editor.update(cx, |editor, cx| {
                                editor.set_placeholder(placeholder, window, cx);
                                editor.set_value(value, window, cx);
                            });
                            probe.show_documentation = true;
                            probe.rendered = false;
                            cx.notify();
                        });
                    })
                    .expect("update native probe");
                gpui::Timer::after(Duration::from_millis(300)).await;
                window
                    .update(cx, |root, _, cx| {
                        let view = root.view().clone().downcast::<Probe>().expect("probe root");
                        assert!(
                            view.read(cx).rendered,
                            "native editor did not render this case"
                        );
                    })
                    .expect("check native render");
            }
            eprintln!("PASS: native Documentation placeholder transitions");
            cx.update(|cx| cx.quit()).expect("quit native probe");
        })
        .detach();
    });
}
