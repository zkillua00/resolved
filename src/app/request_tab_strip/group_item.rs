use super::{super::*, tab_item::request_tab_group_color};

pub(super) fn render_request_tab_group(
    app: &ApiTester,
    group: &RequestTabGroup,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    let group_id = group.id().clone();
    let toggle_id = group_id.clone();
    let context_id = group_id.clone();
    let color = request_tab_group_color(group.color(), cx);
    let active = app
        .request_tabs
        .active()
        .group_id()
        .is_some_and(|active_group_id| active_group_id == group.id());
    let count = app.request_tabs.tabs_in_group(group.id()).len();
    let label = compact_label(group.display_title(), 18);
    let tooltip = format!("{} · {count} tabs", group.display_title());
    let group_key = group.id().as_str();

    h_flex()
        .id(SharedString::from(format!("request-tab-group-{group_key}")))
        .h(px(30.))
        .mx_1()
        .px_2()
        .gap_1()
        .flex_shrink_0()
        .rounded_md()
        .border_1()
        .border_color(color.opacity(if active { 0.8 } else { 0.45 }))
        .bg(color.opacity(if active { 0.18 } else { 0.10 }))
        .text_color(color)
        .cursor_pointer()
        .hover(|style| style.bg(color.opacity(0.24)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, _, _, _| {
                this.request_tab_context_target =
                    Some(super::RequestTabContextTarget::Group(context_id.clone()));
            }),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.toggle_request_tab_group_collapsed(toggle_id.clone(), cx);
        }))
        .child(div().size(px(7.)).rounded_full().flex_shrink_0().bg(color))
        .child(
            div()
                .id(SharedString::from(format!(
                    "request-tab-group-title-{group_key}"
                )))
                .min_w_0()
                .max_w(px(150.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_xs()
                .font_semibold()
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .child(label),
        )
        .child(
            div()
                .text_xs()
                .text_color(color.opacity(0.78))
                .child(count.to_string()),
        )
        .child(
            Icon::new(if group.is_collapsed() {
                IconName::ChevronRight
            } else {
                IconName::ChevronDown
            })
            .xsmall(),
        )
        .into_any_element()
}
