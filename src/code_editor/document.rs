//! Immutable, sparsely indexed response text. Construct on a worker thread.
//!
//! Offsets refer to the UTF-8 document (after lossy decoding, if necessary),
//! not the original binary response. Columns count Unicode display cells, with
//! tabs expanded to tab stops; CRLF is one indivisible newline. No per-line index
//! is retained, even for documents containing millions of empty lines.

use std::{
    fs::File,
    io::{BufWriter, Write},
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use memmap2::Mmap;
use unicode_width::UnicodeWidthChar as _;

use crate::core::ResponseBody;

const STRIDE: usize = 16 * 1024;

#[derive(Clone)]
pub struct Document(Arc<Inner>);

struct Inner {
    storage: Storage,
    checkpoints: Vec<Cursor>,
    tab_size: usize,
    wrap: Option<usize>,
    rows: usize,
    columns: usize,
}

enum Storage {
    Original(ResponseBody),
    Normalized {
        // Field order deliberately drops the mapping before the file.
        mapping: Mmap,
        _file: File,
    },
}

impl Storage {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Original(body) => body,
            Self::Normalized { mapping, .. } => mapping,
        }
    }

    fn text(&self) -> &str {
        // SAFETY: storage is immutable and validated (or normalized) once by
        // `prepare_storage`. Revalidating here would make every seek O(file).
        unsafe { std::str::from_utf8_unchecked(self.bytes()) }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub row: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Default)]
struct Cursor {
    byte: usize,
    utf16: usize,
    row: usize,
    column: usize,
    line: usize,
    line_start: usize,
    run_start: usize,
    class: u8,
    in_string: bool,
    escaped: bool,
}

/// A bounded fragment of one display row. `line_number` is one-based.
/// The source range excludes newline bytes. A partially visible tab still
/// refers to the whole source tab. The map exists only for this fragment.
#[derive(Clone, Debug)]
pub struct Row {
    pub range: Range<usize>,
    pub text: String,
    pub line_number: usize,
    pub start_column: usize,
    source_offsets: Vec<usize>,
}

impl Row {
    /// Hit testing using a rendered UTF-8 byte index (including the end).
    /// Interior bytes of a scalar map to that scalar's source start.
    pub fn source_offset(&self, rendered_byte: usize) -> usize {
        self.source_offsets[rendered_byte.min(self.text.len())]
    }

    /// The first rendered byte for a source offset. Tabs are atomic: their
    /// source start maps before all expansion spaces, and their end after.
    pub fn rendered_offset(&self, source_offset: usize) -> usize {
        self.source_offsets
            .partition_point(|offset| *offset < source_offset)
            .min(self.text.len())
    }
}

impl Document {
    pub fn new(
        body: ResponseBody,
        tab_size: usize,
        wrap_columns: Option<usize>,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let storage = prepare_storage(body, cancel)?;
        let mut inner = Inner {
            storage,
            checkpoints: Vec::new(),
            tab_size: tab_size.max(1),
            wrap: wrap_columns.map(|n| n.max(1)),
            rows: 1,
            columns: 0,
        };
        let mut cursor = Cursor::default();
        cursor.class = inner.class_at(0);
        inner.checkpoints.push(cursor);
        while cursor.byte < inner.storage.bytes().len() {
            if cursor.byte - inner.checkpoints.last().unwrap().byte >= STRIDE {
                check_cancel(cancel)?;
                inner.checkpoints.push(cursor);
            }
            // Downloaded hex/base64 and long JSON strings commonly contain
            // enormous ASCII word runs. Scan one checkpoint span at a time
            // rather than performing Unicode/position queries per byte.
            let until_checkpoint = STRIDE - (cursor.byte - inner.checkpoints.last().unwrap().byte);
            if inner.advance_ascii_word(&mut cursor, until_checkpoint) {
                inner.columns = inner.columns.max(cursor.column);
                continue;
            }
            inner.columns = inner.columns.max(cursor.column + inner.width(cursor));
            inner.advance(&mut cursor);
        }
        check_cancel(cancel)?;
        inner.rows = cursor.row + 1;
        inner.columns = inner.columns.max(cursor.column);
        if inner.checkpoints.last().unwrap().byte != cursor.byte {
            inner.checkpoints.push(cursor);
        }
        Ok(Self(Arc::new(inner)))
    }

    pub fn len(&self) -> usize {
        self.0.storage.bytes().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn all_text(&self) -> &str {
        self.0.storage.text()
    }
    /// Like string indexing: callers must supply valid UTF-8 boundaries.
    pub fn text(&self, range: Range<usize>) -> &str {
        &self.all_text()[range]
    }
    pub fn display_rows(&self) -> usize {
        self.0.rows
    }
    pub fn max_columns(&self) -> usize {
        self.0.columns
    }

    fn boundary(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.len());
        while !self.all_text().is_char_boundary(offset) {
            offset -= 1;
        }
        if offset > 0
            && self.all_text().as_bytes().get(offset) == Some(&b'\n')
            && self.all_text().as_bytes()[offset - 1] == b'\r'
        {
            offset -= 1;
        }
        offset
    }

    fn cursor(&self, offset: usize) -> Cursor {
        let offset = self.boundary(offset);
        let index = self.0.checkpoints.partition_point(|c| c.byte <= offset) - 1;
        let mut cursor = self.0.checkpoints[index];
        while cursor.byte < offset {
            self.0.advance(&mut cursor);
        }
        cursor
    }

    pub fn position(&self, offset: usize) -> Position {
        let c = self.cursor(offset);
        Position {
            row: c.row,
            column: c.column,
        }
    }

    fn at(&self, row: usize, column: usize) -> Cursor {
        let row = row.min(self.display_rows() - 1);
        let i = self
            .0
            .checkpoints
            .partition_point(|c| (c.row, c.column) <= (row, column));
        let mut c = self.0.checkpoints[i.saturating_sub(1)];
        while c.byte < self.len() {
            if c.row == row && c.column >= column {
                break;
            }
            if c.row == row && column < c.column + self.0.width(c) {
                break;
            }
            let mut next = c;
            self.0.advance(&mut next);
            if next.row > row {
                if self.0.token(c.byte).0 != '\n' {
                    c.column += self.0.width(c);
                    c.byte = next.byte;
                    c.utf16 = next.utf16;
                }
                break;
            }
            if next.row == row && next.column > column {
                break;
            }
            c = next;
        }
        c
    }

    pub fn offset_at(&self, row: usize, column: usize) -> usize {
        self.at(row, column).byte
    }

    pub fn line_number_at(&self, offset: usize) -> usize {
        self.cursor(offset).line + 1
    }

    pub fn is_line_start(&self, offset: usize) -> bool {
        self.cursor(offset).line_start == offset
    }

    pub fn json_state(&self, offset: usize) -> (bool, bool) {
        let cursor = self.cursor(offset);
        (cursor.in_string, cursor.escaped)
    }

    /// JSON lexical state immediately before the scalar at `offset`.
    /// This is quote/escape tracking, not a validating JSON parser.
    #[cfg(test)]
    pub fn json_in_string(&self, offset: usize) -> bool {
        self.json_state(offset).0
    }

    pub fn row(&self, row: usize, start_column: usize, max_columns: usize) -> Row {
        let mut c = self.at(row, start_column);
        let actual_row = c.row;
        let start = c.byte;
        let mut result = Row {
            range: start..start,
            text: String::new(),
            line_number: c.line + 1,
            start_column: start_column.min(c.column),
            source_offsets: Vec::new(),
        };
        let stop = start_column.saturating_add(max_columns);
        if max_columns == 0
            || self.position(c.byte).row != actual_row
            || c.column < start_column
                && (c.byte == self.len() || c.column + self.0.width(c) <= start_column)
        {
            result.source_offsets.push(start);
            return result;
        }
        while c.byte < self.len()
            && c.row == actual_row
            && (c.column < stop || self.0.width(c) == 0)
        {
            let (ch, _) = self.0.token(c.byte);
            if ch == '\n' {
                break;
            }
            let width = self.0.width(c);
            if ch == '\t' {
                let visible_start = c.column.max(start_column);
                let visible_end = (c.column + width).min(stop);
                if result.text.is_empty() {
                    result.start_column = visible_start.min(stop);
                }
                for _ in visible_start..visible_end {
                    result.text.push(' ');
                    result.source_offsets.push(c.byte);
                }
            } else {
                result.text.push(ch);
                result
                    .source_offsets
                    .extend(std::iter::repeat_n(c.byte, ch.len_utf8()));
            }
            self.0.advance(&mut c);
            result.range.end = c.byte;
        }
        result.source_offsets.push(result.range.end);
        result
    }

    pub fn previous_offset(&self, offset: usize) -> usize {
        self.boundary(self.boundary(offset).saturating_sub(1))
    }

    pub fn next_offset(&self, offset: usize) -> usize {
        let offset = self.boundary(offset);
        if offset == self.len() {
            offset
        } else {
            offset + self.0.token(offset).1
        }
    }

    pub fn line_range(&self, offset: usize) -> Range<usize> {
        let c = self.cursor(offset);
        let i = self
            .0
            .checkpoints
            .partition_point(|next| next.line <= c.line)
            - 1;
        let mut end = self.0.checkpoints[i];
        if end.byte < c.byte {
            end = c;
        }
        while end.byte < self.len() && end.line == c.line {
            self.0.advance(&mut end);
        }
        c.line_start..end.byte
    }

    /// Words are maximal runs of alphanumeric/underscore, whitespace, or
    /// punctuation scalars. Checkpoints permit even enormous runs to be skipped.
    pub fn word_range(&self, offset: usize) -> Range<usize> {
        let c = self.cursor(offset);
        if c.byte == self.len() {
            return c.byte..c.byte;
        }
        let i = self
            .0
            .checkpoints
            .partition_point(|next| next.run_start <= c.run_start)
            - 1;
        let mut end = self.0.checkpoints[i];
        if end.byte < c.byte {
            end = c;
        }
        while end.byte < self.len() && end.run_start == c.run_start {
            self.0.advance(&mut end);
        }
        c.run_start..end.byte
    }

    pub fn word_left(&self, offset: usize) -> usize {
        self.cursor(self.previous_offset(offset)).run_start
    }

    pub fn word_right(&self, offset: usize) -> usize {
        self.word_range(offset).end
    }

    pub fn utf16_offset(&self, byte_offset: usize) -> usize {
        // UTF-16 conversion preserves the position between CR and LF, unlike
        // cursor navigation, since platform text APIs can request that index.
        let mut offset = byte_offset.min(self.len());
        while !self.all_text().is_char_boundary(offset) {
            offset -= 1;
        }
        let c = self.cursor(offset);
        c.utf16 + usize::from(c.byte != offset)
    }

    pub fn byte_offset(&self, utf16_offset: usize) -> usize {
        let i = self
            .0
            .checkpoints
            .partition_point(|c| c.utf16 <= utf16_offset);
        let mut c = self.0.checkpoints[i.saturating_sub(1)];
        while c.byte < self.len() && c.utf16 < utf16_offset {
            let ch = self.all_text()[c.byte..].chars().next().unwrap();
            if c.utf16 + ch.len_utf16() > utf16_offset {
                break;
            }
            c.byte += ch.len_utf8();
            c.utf16 += ch.len_utf16();
        }
        c.byte
    }
}

impl Inner {
    fn advance_ascii_word(&self, cursor: &mut Cursor, maximum: usize) -> bool {
        // Wrapped layout must account for visual-row transitions. The scalar
        // path below is used there; this fast path is for unwrapped documents.
        if self.wrap.is_some() || cursor.class != 1 || maximum == 0 {
            return false;
        }
        let bytes = self.storage.bytes();
        let end = cursor.byte.saturating_add(maximum).min(bytes.len());
        let len = bytes[cursor.byte..end]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
            .count();
        if len == 0 {
            return false;
        }
        cursor.byte += len;
        cursor.utf16 += len;
        cursor.column += len;
        cursor.escaped = false;
        let class = self.class_at(cursor.byte);
        if class != cursor.class {
            cursor.run_start = cursor.byte;
            cursor.class = class;
        }
        true
    }

    fn token(&self, byte: usize) -> (char, usize) {
        let text = &self.storage.text()[byte..];
        if text.starts_with("\r\n") {
            ('\n', 2)
        } else {
            let ch = text.chars().next().unwrap();
            (ch, ch.len_utf8())
        }
    }

    fn class_at(&self, byte: usize) -> u8 {
        if byte == self.storage.bytes().len() {
            return 0;
        }
        let ch = self.token(byte).0;
        if ch.is_alphanumeric() || ch == '_' {
            1
        } else if ch.is_whitespace() {
            2
        } else {
            3
        }
    }

    fn width(&self, c: Cursor) -> usize {
        if c.byte == self.storage.bytes().len() {
            return 0;
        }
        match self.token(c.byte).0 {
            '\n' => 0,
            '\t' => (self.tab_size - c.column % self.tab_size).min(self.wrap.unwrap_or(usize::MAX)),
            ch => ch.width().unwrap_or(1),
        }
    }

    fn advance(&self, c: &mut Cursor) {
        let (ch, bytes) = self.token(c.byte);
        let width = self.width(*c);
        if c.escaped {
            c.escaped = false;
        } else if ch == '"' {
            c.in_string = !c.in_string;
        } else if ch == '\\' && c.in_string {
            c.escaped = true;
        }
        c.byte += bytes;
        c.utf16 += if ch == '\n' { bytes } else { ch.len_utf16() };
        if ch == '\n' {
            c.row += 1;
            c.column = 0;
            c.line += 1;
            c.line_start = c.byte;
        } else {
            c.column += width;
            if let Some(wrap) = self.wrap {
                if c.byte < self.storage.bytes().len()
                    && self.token(c.byte).0 != '\n'
                    && (c.column >= wrap && self.width(*c) > 0
                        || self.width(*c) > wrap.saturating_sub(c.column))
                {
                    c.row += 1;
                    c.column = 0;
                }
            }
        }
        let class = self.class_at(c.byte);
        if class != c.class {
            c.run_start = c.byte;
            c.class = class;
        }
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Document indexing cancelled".into())
    } else {
        Ok(())
    }
}

/// Validate in bounded chunks. Once an error is encountered, stream the valid
/// prefix and lossy-decoded remainder into a private anonymous temporary file.
fn prepare_storage(body: ResponseBody, cancel: &AtomicBool) -> Result<Storage, String> {
    let mut offset = 0;
    let mut writer: Option<BufWriter<File>> = None;
    while offset < body.len() {
        check_cancel(cancel)?;
        let end = offset.saturating_add(STRIDE).min(body.len());
        match std::str::from_utf8(&body[offset..end]) {
            Ok(valid) => {
                if let Some(writer) = writer.as_mut() {
                    writer
                        .write_all(valid.as_bytes())
                        .map_err(|e| e.to_string())?;
                }
                offset = end;
            }
            Err(error) => {
                let valid_end = offset + error.valid_up_to();
                if let Some(writer) = writer.as_mut() {
                    writer
                        .write_all(&body[offset..valid_end])
                        .map_err(|e| e.to_string())?;
                }
                if error.error_len().is_none() && end < body.len() {
                    offset = valid_end;
                    continue;
                }
                if writer.is_none() {
                    let mut output =
                        BufWriter::new(tempfile::tempfile().map_err(|e| e.to_string())?);
                    // Chunk the prefix too, so cancellation stays responsive.
                    for chunk in body[..valid_end].chunks(STRIDE) {
                        check_cancel(cancel)?;
                        output.write_all(chunk).map_err(|e| e.to_string())?;
                    }
                    writer = Some(output);
                }
                writer
                    .as_mut()
                    .unwrap()
                    .write_all("\u{fffd}".as_bytes())
                    .map_err(|e| e.to_string())?;
                offset = valid_end + error.error_len().unwrap_or(end - valid_end);
            }
        }
    }
    check_cancel(cancel)?;
    match writer {
        None => Ok(Storage::Original(body)),
        Some(writer) => {
            let file = writer.into_inner().map_err(|e| e.to_string())?;
            // SAFETY: this anonymous file is private, fully written, never
            // modified again, and owned alongside its mapping until drop.
            let mapping = unsafe { Mmap::map(&file) }.map_err(|e| e.to_string())?;
            Ok(Storage::Normalized {
                mapping,
                _file: file,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str, wrap: Option<usize>) -> Document {
        Document::new(
            text.as_bytes().to_vec().into(),
            4,
            wrap,
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn empty_eof_and_trailing_newline() {
        let empty = doc("", None);
        assert!(empty.is_empty());
        assert_eq!(empty.display_rows(), 1);
        assert_eq!(empty.offset_at(100, 100), 0);
        assert_eq!(empty.line_range(0), 0..0);
        assert_eq!(empty.row(0, 0, 20).source_offset(0), 0);
        let d = doc("a\n", None);
        assert_eq!(d.display_rows(), 2);
        assert_eq!(d.line_range(0), 0..2);
        assert_eq!(d.line_range(2), 2..2);
        assert_eq!(d.position(2), Position { row: 1, column: 0 });
    }

    #[test]
    fn unicode_crlf_and_utf16() {
        let d = doc("é😀\r\n中\tz", None);
        assert_eq!(d.next_offset(0), 2);
        assert_eq!(d.next_offset(2), 6);
        assert_eq!(d.next_offset(6), 8);
        assert_eq!(d.previous_offset(8), 6);
        assert_eq!(d.position(7), Position { row: 0, column: 3 });
        assert_eq!(d.line_range(9), 8..13);
        for (byte, utf16) in [
            (0, 0),
            (2, 1),
            (6, 3),
            (7, 4),
            (8, 5),
            (11, 6),
            (12, 7),
            (13, 8),
        ] {
            assert_eq!(d.utf16_offset(byte), utf16);
            assert_eq!(d.byte_offset(utf16), byte);
        }
        assert_eq!(d.byte_offset(2), 2);
        assert_eq!(d.byte_offset(usize::MAX), d.len());
    }

    #[test]
    fn viewport_tabs_and_bidirectional_mapping() {
        let d = doc("é\t😀z", None);
        let r = d.row(0, 0, 10);
        assert_eq!(r.text, "é   😀z");
        assert_eq!(r.start_column, 0);
        assert_eq!(r.source_offset(1), 0);
        assert_eq!(r.source_offset(4), 2);
        assert_eq!(r.rendered_offset(2), 2);
        assert_eq!(r.rendered_offset(3), 5);
        assert_eq!(r.source_offset(r.text.len()), d.len());
        let clipped = d.row(0, 2, 3);
        assert_eq!(clipped.text, "  😀");
        assert_eq!(clipped.start_column, 2);
        assert_eq!(clipped.range, 2..7);
        assert_eq!(d.offset_at(0, 3), 2);
        assert_eq!(d.row(0, 100, 5).text, "");
        assert_eq!(d.row(0, 2, 0).text, "");
    }

    #[test]
    fn wrap_does_not_insert_bytes_or_phantom_rows() {
        let d = doc("abcd\nefghi\tj", Some(4));
        assert_eq!(d.all_text(), "abcd\nefghi\tj");
        assert_eq!(d.row(0, 0, 10).text, "abcd");
        assert_eq!(d.row(1, 0, 10).text, "efgh");
        assert_eq!(d.row(2, 0, 10).text, "i   ");
        assert_eq!(d.row(3, 0, 10).text, "j");
        assert_eq!(d.display_rows(), 4);
        assert_eq!(d.max_columns(), 4);
        assert_eq!(d.offset_at(1, 100), 9);
        assert_eq!(d.row(1, 100, 10).text, "");
        assert_eq!(d.line_range(10), 5..12);
        assert_eq!(doc("abcd", Some(4)).display_rows(), 1);
        assert_eq!(doc("abcd\n", Some(4)).display_rows(), 2);
        assert_eq!(doc("\tX", Some(2)).row(0, 1, 1).text, " ");
    }

    #[test]
    fn sparse_giant_line_and_many_lines() {
        let text = "a".repeat(2 * 1024 * 1024);
        let d = doc(&text, None);
        assert!(d.0.checkpoints.len() <= text.len() / STRIDE + 2);
        assert_eq!(d.position(text.len() - 3).column, text.len() - 3);
        assert_eq!(d.offset_at(0, text.len() - 3), text.len() - 3);
        assert_eq!(d.word_range(text.len() - 3), 0..text.len());
        assert_eq!(d.line_range(3), 0..text.len());
        assert_eq!(d.row(0, text.len() - 3, 10).text, "aaa");
        assert_eq!(d.text(STRIDE - 20..STRIDE * 5 + 20).len(), STRIDE * 4 + 40);
        let lines = doc(&"\n".repeat(100_000), None);
        assert!(lines.0.checkpoints.len() < 10);
        assert_eq!(lines.offset_at(99_999, 0), 99_999);
        assert_eq!(lines.line_range(99_999), 99_999..100_000);
    }

    #[test]
    fn valid_utf8_is_not_copied_and_clone_retains_storage() {
        let body: ResponseBody = "x😀".repeat(STRIDE).into_bytes().into();
        let pointer = body.as_ptr();
        let d = Document::new(body, 4, None, &AtomicBool::new(false)).unwrap();
        assert_eq!(d.all_text().as_ptr(), pointer);
        let clone = d.clone();
        drop(d);
        assert_eq!(clone.all_text().as_ptr(), pointer);
    }

    #[test]
    fn invalid_utf8_matches_lossy_decode_across_chunks() {
        for prefix in [0, STRIDE - 3, STRIDE - 1, STRIDE, STRIDE + 1] {
            let mut bytes = vec![b'a'; prefix];
            bytes.extend_from_slice(b"\xf0\x9f\x98\x80\xff\xe2\x82X\xc0\xaf\xf0\x90");
            let expected = String::from_utf8_lossy(&bytes).into_owned();
            let d = Document::new(bytes.into(), 4, None, &AtomicBool::new(false)).unwrap();
            assert_eq!(d.all_text(), expected);
            assert!(matches!(d.0.storage, Storage::Normalized { .. }));
        }
    }

    #[test]
    fn cancellation_and_json_checkpoint_state() {
        assert!(Document::new(vec![b'x'; STRIDE].into(), 4, None, &AtomicBool::new(true)).is_err());
        let text = format!("\"{}\\\"still\" false", "a".repeat(STRIDE * 3));
        let d = doc(&text, None);
        assert!(d.json_in_string(STRIDE * 2));
        assert!(d.json_in_string(STRIDE * 3 + 3));
        assert!(!d.json_in_string(d.len() - 2));
        assert_eq!(d.word_left(d.len()), d.len() - 5);
        assert_eq!(d.word_right(d.len() - 3), d.len());
    }
}
