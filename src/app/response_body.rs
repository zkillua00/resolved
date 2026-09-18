use super::*;

const FILE_BACKED_RESPONSE_ROW_BYTES: usize = 4 * 1024;

fn file_backed_row_range(body: &[u8], index: usize) -> std::ops::Range<usize> {
    let boundary_at_or_after = |mut offset: usize| {
        offset = offset.min(body.len());
        while offset < body.len() && body[offset] & 0b1100_0000 == 0b1000_0000 {
            offset += 1;
        }
        offset
    };
    let start = boundary_at_or_after(index.saturating_mul(FILE_BACKED_RESPONSE_ROW_BYTES));
    let end = boundary_at_or_after(
        index
            .saturating_add(1)
            .saturating_mul(FILE_BACKED_RESPONSE_ROW_BYTES),
    );
    start..end
}

pub(super) fn render_file_backed_response(
    id: SharedString,
    response: &ResponseData,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    let body = response.body.clone();
    let row_count = body.len().div_ceil(FILE_BACKED_RESPONSE_ROW_BYTES);
    let mono_font = cx.theme().mono_font_family.clone();
    uniform_list(id, row_count, move |range, _, _| {
        range
            .map(|index| {
                let bytes = &body[file_backed_row_range(&body, index)];
                div()
                    .px_3()
                    .font_family(mono_font.clone())
                    .text_sm()
                    .whitespace_nowrap()
                    .child(String::from_utf8_lossy(bytes).into_owned())
            })
            .collect()
    })
    .size_full()
    .into_any_element()
}

impl ApiTester {
    pub(super) fn render_response_summary(
        &self,
        response: &ResponseData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status_color = status_color(response.status, cx);
        h_flex()
            .gap_3()
            .text_xs()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(status_color.opacity(0.14))
                    .text_color(status_color)
                    .font_semibold()
                    .child(format!("{} {}", response.status, response.status_text)),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(format_duration(response.duration)),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(format_bytes(response.size_bytes())),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(response.http_version.clone()),
            )
            .into_any_element()
    }

    pub(super) fn render_response_body(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(response) = self
            .response
            .as_ref()
            .filter(|response| response.body.is_file_backed() && response_body_is_text(response))
        {
            return div()
                .size_full()
                .bg(cx.api_surface_lowest())
                .child(render_file_backed_response(
                    "file-backed-response-body".into(),
                    response,
                    cx,
                ))
                .into_any_element();
        }
        div()
            .size_full()
            .bg(cx.api_surface_lowest())
            .child(self.response_editor.clone())
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_response_rows_cover_utf8_body_exactly_once() {
        let mut body = vec![b'a'; FILE_BACKED_RESPONSE_ROW_BYTES - 1];
        body.extend_from_slice("🙂middle".as_bytes());
        body.extend(std::iter::repeat_n(b'z', FILE_BACKED_RESPONSE_ROW_BYTES));

        let row_count = body.len().div_ceil(FILE_BACKED_RESPONSE_ROW_BYTES);
        let rebuilt = (0..row_count)
            .flat_map(|index| body[file_backed_row_range(&body, index)].iter().copied())
            .collect::<Vec<_>>();

        assert_eq!(rebuilt, body);
    }
}
