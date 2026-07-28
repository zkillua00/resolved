use super::super::*;

pub(super) fn render_request_tab(
    app: &ApiTester,
    tab: &RequestTabRecord,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    let tab_id = tab.id().clone();
    let activate_id = tab_id.clone();
    let close_id = tab_id.clone();
    let context_id = tab_id.clone();
    let tab_key = tab_id.as_str();
    let row_id: SharedString = format!("open-request-tab-{tab_key}").into();
    let row_group: SharedString = format!("open-request-tab-group-{tab_key}").into();
    let title_id: SharedString = format!("open-request-tab-title-{tab_key}").into();
    let close_button_id: SharedString = format!("close-open-request-tab-{tab_key}").into();
    let active = app.request_tabs.active_tab_id() == &tab_id;
    let dirty = if active {
        app.request_is_dirty()
    } else {
        tab.is_dirty()
    };
    let group_color = tab
        .group_id()
        .and_then(|group_id| app.request_tabs.group(group_id))
        .map(|group| request_tab_group_color(group.color(), cx));
    let title = compact_label(tab.display_title(), 28);
    let tooltip = tab.display_title().to_owned();

    h_flex()
        .id(row_id)
        .group(row_group.clone())
        .relative()
        .h_full()
        .flex_shrink_0()
        .min_w(px(148.))
        .max_w(px(240.))
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
        .when_some(group_color, |this, color| {
            this.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(2.))
                    .bg(color),
            )
        })
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, _, _, _| {
                this.request_tab_context_target =
                    Some(super::RequestTabContextTarget::Tab(context_id.clone()));
            }),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            this.activate_request_tab(activate_id.clone(), window, cx);
        }))
        .child(
            div()
                .id(title_id)
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_sm()
                .font_medium()
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .child(title),
        )
        .when(dirty, |this| {
            this.child(
                div()
                    .size(px(7.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(cx.theme().warning),
            )
        })
        .child(
            Button::new(close_button_id)
                .icon(IconName::Close)
                .xsmall()
                .ghost()
                .rounded_full()
                .tooltip("Close request tab")
                .when(!active, |this| {
                    this.invisible()
                        .group_hover(row_group, |style| style.visible())
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.request_close_request_tab(close_id.clone(), window, cx);
                })),
        )
        .into_any_element()
}

pub(super) fn request_tab_group_color(color: &RequestTabGroupColor, cx: &App) -> Hsla {
    match color {
        RequestTabGroupColor::Gray => cx.theme().muted_foreground,
        RequestTabGroupColor::Blue => cx.theme().blue,
        RequestTabGroupColor::Cyan => cx.theme().cyan,
        RequestTabGroupColor::Green => cx.theme().green,
        RequestTabGroupColor::Yellow => cx.theme().yellow,
        RequestTabGroupColor::Orange => cx.theme().warning,
        RequestTabGroupColor::Red => cx.theme().red,
        RequestTabGroupColor::Pink => cx.theme().magenta,
        RequestTabGroupColor::Purple => cx.theme().primary,
        RequestTabGroupColor::Custom(_) => cx.theme().primary,
    }
}
