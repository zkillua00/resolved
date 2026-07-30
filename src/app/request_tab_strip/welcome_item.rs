use super::super::*;

pub(super) fn render_welcome_tab(app: &ApiTester, cx: &mut Context<ApiTester>) -> AnyElement {
    let active = app.workspace_tabs.active() == ActiveWorkspaceTab::Welcome;

    h_flex()
        .id("workspace-welcome-tab")
        .relative()
        .h_full()
        .flex_shrink_0()
        .min_w(px(148.))
        .max_w(px(220.))
        .px_3()
        .gap_2()
        .border_r_1()
        .border_color(cx.api_outline_variant())
        .cursor_pointer()
        .when(active, |this| {
            this.bg(cx.api_surface())
                .border_b_2()
                .border_color(cx.theme().primary)
        })
        .when(!active, |this| {
            this.bg(cx.api_surface_low())
                .hover(|style| style.bg(cx.api_surface_container()))
        })
        .on_click(cx.listener(|this, _, _, cx| {
            if this.workspace_tabs.activate_welcome() {
                this.hide_preview(cx);
                cx.notify();
            }
        }))
        .child(Icon::new(IconName::GalleryVerticalEnd).xsmall())
        .child(div().text_sm().font_medium().child("Welcome"))
        .into_any_element()
}
