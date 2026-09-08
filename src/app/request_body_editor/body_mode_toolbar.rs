use super::super::*;

impl ApiTester {
    pub(super) fn render_body_mode_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode_switch = TabBar::new("body-mode")
            .segmented()
            .small()
            .children(BodyMode::all().iter().map(|mode| mode.label()))
            .selected_index(
                BodyMode::all()
                    .iter()
                    .position(|mode| *mode == self.body_mode)
                    .unwrap_or(0),
            )
            .on_click(cx.listener(move |this, index: &usize, window, cx| {
                if let Some(&mode) = BodyMode::all().get(*index) {
                    this.select_body_mode(mode, window, cx);
                }
            }));

        let selected_language = self.raw_body_language;
        let this = cx.entity().downgrade();
        let language_selector = Button::new("raw-body-language")
            .label(selected_language.label())
            .dropdown_caret(true)
            .small()
            .outline()
            .rounded(px(18.))
            .dropdown_menu(move |menu, _, _| {
                RawBodyLanguage::all().iter().copied().fold(
                    menu.min_w(px(190.)).max_h(px(420.)).scrollable(true),
                    |menu, language| {
                        let this = this.clone();
                        menu.item(
                            PopupMenuItem::new(language.label())
                                .checked(language == selected_language)
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.select_raw_body_language(language, cx);
                                        });
                                    }
                                }),
                        )
                    },
                )
            });

        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(mode_switch)
                    .when(self.body_mode == BodyMode::Raw, |this| {
                        this.child(language_selector)
                    }),
            )
            .into_any_element()
    }
}
