use super::super::*;

impl ApiTester {
    pub(super) fn render_body_mode_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode_buttons = BodyMode::all()
            .iter()
            .copied()
            .enumerate()
            .map(|(index, mode)| {
                Button::new(("body-mode", index))
                    .label(mode.label())
                    .small()
                    .ghost()
                    .rounded(px(18.))
                    .selected(self.body_mode == mode)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_body_mode(mode, window, cx);
                    }))
            })
            .collect::<Vec<_>>();

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
                    .child(h_flex().flex_wrap().gap_1().children(mode_buttons))
                    .when(self.body_mode == BodyMode::Raw, |this| {
                        this.child(language_selector)
                    }),
            )
            .into_any_element()
    }
}
