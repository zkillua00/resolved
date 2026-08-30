use super::*;

impl ApiTester {
    pub(super) fn render_environment_title_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active_environment_full = self
            .workspace
            .active_environment()
            .map(|environment| environment.name.clone())
            .unwrap_or_else(|| "No environment".to_owned());
        let active_environment = compact_label(&active_environment_full, 30);
        let active_environment_id = self.workspace.active_environment_id.clone();
        let environments = self
            .workspace
            .environments
            .iter()
            .map(|environment| (environment.id.clone(), environment.name.clone()))
            .collect::<Vec<_>>();
        let selected_name = self
            .selected_environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id))
            .map(|environment| environment.name.as_str())
            .unwrap_or("No environment selected");
        let can_switch_environment = self.can_select_environment() && !self.sending;
        let this = cx.entity().downgrade();

        h_flex()
            .h(px(APP_TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .pl(windows_controls::leading_inset())
            .pr(windows_controls::trailing_inset())
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
            .bg(cx.theme().title_bar)
            .child(
                h_flex()
                    .min_w_0()
                    .gap_6()
                    .child(resolved_brand_lockup(cx))
                    .child(
                        h_flex()
                            .h_full()
                            .items_center()
                            .border_b_2()
                            .border_color(cx.theme().primary)
                            .px_1()
                            .text_sm()
                            .font_semibold()
                            .child("Environments"),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .max_w(px(420.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(selected_name.to_owned()),
                    ),
            )
            .child(div().flex_1())
            .child(
                Button::new("environment-workspace-active")
                    .icon(IconName::Globe)
                    .label(active_environment)
                    .large()
                    .h(px(38.))
                    .outline()
                    .rounded(px(20.))
                    .tooltip(format!("Active environment: {active_environment_full}"))
                    .dropdown_menu(move |menu, _, _| {
                        let no_environment_this = this.clone();
                        let menu = menu.min_w(px(220.)).item(
                            PopupMenuItem::new("No environment")
                                .checked(active_environment_id.is_none())
                                .disabled(!can_switch_environment)
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = no_environment_this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.activate_environment(None, cx);
                                        });
                                    }
                                }),
                        );
                        environments.iter().fold(menu, |menu, (id, name)| {
                            let environment_id = id.clone();
                            let checked = active_environment_id.as_deref() == Some(id.as_str());
                            let environment_this = this.clone();
                            menu.item(
                                PopupMenuItem::new(name.clone())
                                    .checked(checked)
                                    .disabled(!can_switch_environment)
                                    .on_click(move |_, _, cx| {
                                        if let Some(this) = environment_this.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.activate_environment(
                                                    Some(environment_id.clone()),
                                                    cx,
                                                );
                                            });
                                        }
                                    }),
                            )
                        })
                    }),
            )
            .child(windows_controls::windows_window_controls(window, cx))
            .into_any_element()
    }
}
