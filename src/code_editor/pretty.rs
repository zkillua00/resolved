//! Whitespace-only JSON formatting for mapped response bodies.
//!
//! Tokens are borrowed from the input mapping and copied in bounded chunks.
//! The only variable-sized working storage is an explicit O(nesting depth)
//! grammar stack: neither long strings/numbers nor wide containers allocate.
//! Unlike a recursive deserializer this has no artificial nesting limit, and
//! unlike parsing numbers as floats it preserves every digit and exponent.

use std::{
    io::{self, BufWriter, Write},
    sync::atomic::{AtomicBool, Ordering},
};

use crate::core::{FormatterSettings, ResponseBody};

const CHUNK: usize = 16 * 1024;

/// Return a complete anonymous file-backed JSON representation, or the original
/// shared body if any part is not JSON. No partially formatted result escapes.
pub(super) fn pretty_body(
    body: &ResponseBody,
    settings: &FormatterSettings,
    cancelled: &AtomicBool,
) -> Result<ResponseBody, String> {
    check_cancelled(cancelled).map_err(|error| error.to_string())?;
    // Validate without producing indentation first. Invalid input must not
    // write an arbitrarily expanded speculative representation to disk.
    let validation = Formatter {
        input: body.as_ref(),
        position: 0,
        next_check: 0,
        output: &mut io::sink(),
        cancelled,
        indent: 0,
        indent_byte: b' ',
        stack: vec![State::RootValue],
    }
    .run();
    match validation {
        Err(Error::Invalid) => {
            check_cancelled(cancelled).map_err(|error| error.to_string())?;
            return Ok(body.clone());
        }
        Err(error) => return Err(error.to_string()),
        Ok(()) => {}
    }
    let file = tempfile::tempfile().map_err(|error| error.to_string())?;
    let mut output = BufWriter::with_capacity(CHUNK, file);
    let result = Formatter {
        input: body.as_ref(),
        position: 0,
        next_check: 0,
        output: &mut output,
        cancelled,
        indent: if settings.hard_tabs {
            1
        } else {
            settings.effective_indent_size()
        },
        indent_byte: if settings.hard_tabs { b'\t' } else { b' ' },
        stack: vec![State::RootValue],
    }
    .run();
    match result {
        Err(Error::Invalid) => {
            // Discard speculative buffered bytes without a flush on drop.
            let _ = output.into_parts();
            // Cancellation wins even if malformed input was encountered first.
            check_cancelled(cancelled).map_err(|error| error.to_string())?;
            Ok(body.clone())
        }
        Err(error) => {
            let _ = output.into_parts();
            Err(error.to_string())
        }
        Ok(()) => {
            check_cancelled(cancelled).map_err(|error| error.to_string())?;
            output.flush().map_err(|error| error.to_string())?;
            let file = output.into_inner().map_err(|error| error.to_string())?;
            check_cancelled(cancelled).map_err(|error| error.to_string())?;
            // SAFETY: the anonymous output is fully flushed, and neither its
            // handle nor any writable mapping has been shared.
            unsafe { ResponseBody::file(file) }.map_err(|error| error.to_string())
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    RootValue,
    RootEnd,
    ArrayFirst,
    ArrayValue,
    ArrayAfter,
    ObjectFirst,
    ObjectKey,
    ObjectColon,
    ObjectValue,
    ObjectAfter,
}

#[derive(Debug)]
enum Error {
    Invalid,
    Cancelled,
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => f.write_str("Invalid JSON"),
            Self::Cancelled => f.write_str("Response formatting cancelled"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), Error> {
    if cancelled.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

struct Formatter<'a, W> {
    input: &'a [u8],
    position: usize,
    next_check: usize,
    output: &'a mut W,
    cancelled: &'a AtomicBool,
    indent: usize,
    indent_byte: u8,
    stack: Vec<State>,
}

impl<W: Write> Formatter<'_, W> {
    fn run(mut self) -> Result<(), Error> {
        loop {
            self.whitespace()?;
            match *self.stack.last().ok_or(Error::Invalid)? {
                State::RootValue => {
                    self.replace(State::RootEnd);
                    self.value()?;
                }
                State::RootEnd => {
                    return if self.position == self.input.len() {
                        Ok(())
                    } else {
                        Err(Error::Invalid)
                    };
                }
                State::ArrayFirst if self.peek() == Some(b']') => self.close(b']', false)?,
                State::ArrayFirst | State::ArrayValue => {
                    self.newline(self.stack.len() - 1)?;
                    self.replace(State::ArrayAfter);
                    self.value()?;
                }
                State::ArrayAfter => match self.peek() {
                    Some(b',') => {
                        self.punctuation(b",")?;
                        self.replace(State::ArrayValue);
                    }
                    Some(b']') => self.close(b']', true)?,
                    _ => return Err(Error::Invalid),
                },
                State::ObjectFirst if self.peek() == Some(b'}') => self.close(b'}', false)?,
                State::ObjectFirst | State::ObjectKey => {
                    self.newline(self.stack.len() - 1)?;
                    self.string()?;
                    self.replace(State::ObjectColon);
                }
                State::ObjectColon => {
                    if self.peek() != Some(b':') {
                        return Err(Error::Invalid);
                    }
                    self.punctuation(b": ")?;
                    self.replace(State::ObjectValue);
                }
                State::ObjectValue => {
                    self.replace(State::ObjectAfter);
                    self.value()?;
                }
                State::ObjectAfter => match self.peek() {
                    Some(b',') => {
                        self.punctuation(b",")?;
                        self.replace(State::ObjectKey);
                    }
                    Some(b'}') => self.close(b'}', true)?,
                    _ => return Err(Error::Invalid),
                },
            }
        }
    }

    fn replace(&mut self, state: State) {
        *self.stack.last_mut().expect("root state remains present") = state;
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    fn checkpoint(&mut self) -> Result<(), Error> {
        if self.position >= self.next_check {
            check_cancelled(self.cancelled)?;
            self.next_check = self.position.saturating_add(CHUNK);
        }
        Ok(())
    }

    fn whitespace(&mut self) -> Result<(), Error> {
        self.checkpoint()?;
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
            self.checkpoint()?;
        }
        Ok(())
    }

    fn punctuation(&mut self, text: &[u8]) -> Result<(), Error> {
        self.output.write_all(text)?;
        self.position += 1;
        Ok(())
    }

    fn close(&mut self, byte: u8, nonempty: bool) -> Result<(), Error> {
        self.stack.pop();
        if nonempty {
            self.newline(self.stack.len() - 1)?;
        }
        self.punctuation(&[byte])
    }

    fn newline(&mut self, depth: usize) -> Result<(), Error> {
        self.output.write_all(b"\n")?;
        // Settings store indentation width as a u8.
        let block = [self.indent_byte; 256];
        // Loop over levels instead of multiplying depth by indent width, so
        // arbitrarily deep input cannot overflow an indentation length.
        for _ in 0..depth {
            check_cancelled(self.cancelled)?;
            self.output.write_all(&block[..self.indent])?;
        }
        Ok(())
    }

    fn copy_token(&mut self, start: usize) -> Result<(), Error> {
        for bytes in self.input[start..self.position].chunks(CHUNK) {
            check_cancelled(self.cancelled)?;
            self.output.write_all(bytes)?;
        }
        Ok(())
    }

    fn value(&mut self) -> Result<(), Error> {
        match self.peek() {
            Some(b'{') => {
                self.punctuation(b"{")?;
                self.stack.push(State::ObjectFirst);
            }
            Some(b'[') => {
                self.punctuation(b"[")?;
                self.stack.push(State::ArrayFirst);
            }
            Some(b'"') => self.string()?,
            Some(b't') => self.literal(b"true")?,
            Some(b'f') => self.literal(b"false")?,
            Some(b'n') => self.literal(b"null")?,
            Some(b'-' | b'0'..=b'9') => self.number()?,
            _ => return Err(Error::Invalid),
        }
        Ok(())
    }

    fn literal(&mut self, text: &[u8]) -> Result<(), Error> {
        if !self.input[self.position..].starts_with(text) {
            return Err(Error::Invalid);
        }
        self.output.write_all(text)?;
        self.position += text.len();
        Ok(())
    }

    fn digits(&mut self) -> Result<(), Error> {
        let start = self.position;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.position += 1;
            self.checkpoint()?;
        }
        if start == self.position {
            Err(Error::Invalid)
        } else {
            Ok(())
        }
    }

    fn number(&mut self) -> Result<(), Error> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        match self.peek() {
            Some(b'0') => self.position += 1,
            Some(b'1'..=b'9') => self.digits()?,
            _ => return Err(Error::Invalid),
        }
        if self.peek() == Some(b'.') {
            self.position += 1;
            self.digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            self.digits()?;
        }
        self.copy_token(start)
    }

    fn hex_quad(&mut self) -> Result<u16, Error> {
        let mut value = 0;
        for _ in 0..4 {
            let digit = match self.peek() {
                Some(byte @ b'0'..=b'9') => byte - b'0',
                Some(byte @ b'a'..=b'f') => byte - b'a' + 10,
                Some(byte @ b'A'..=b'F') => byte - b'A' + 10,
                _ => return Err(Error::Invalid),
            };
            self.position += 1;
            value = value * 16 + u16::from(digit);
        }
        Ok(value)
    }

    fn string(&mut self) -> Result<(), Error> {
        if self.peek() != Some(b'"') {
            return Err(Error::Invalid);
        }
        let start = self.position;
        self.position += 1;
        loop {
            self.checkpoint()?;
            match self.peek() {
                Some(b'"') => {
                    self.position += 1;
                    return self.copy_token(start);
                }
                Some(b'\\') => {
                    self.position += 1;
                    match self.peek() {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.position += 1;
                        }
                        Some(b'u') => {
                            self.position += 1;
                            match self.hex_quad()? {
                                0xd800..=0xdbff => {
                                    if !self.input[self.position..].starts_with(b"\\u") {
                                        return Err(Error::Invalid);
                                    }
                                    self.position += 2;
                                    if !(0xdc00..=0xdfff).contains(&self.hex_quad()?) {
                                        return Err(Error::Invalid);
                                    }
                                }
                                0xdc00..=0xdfff => return Err(Error::Invalid),
                                _ => {}
                            }
                        }
                        _ => return Err(Error::Invalid),
                    }
                }
                Some(0..=0x1f) | None => return Err(Error::Invalid),
                Some(0x20..=0x7f) => self.position += 1,
                Some(byte) => {
                    let width = match byte {
                        0xc2..=0xdf => 2,
                        0xe0..=0xef => 3,
                        0xf0..=0xf4 => 4,
                        _ => return Err(Error::Invalid),
                    };
                    let remaining = &self.input[self.position..];
                    let scalar = remaining.get(..width).ok_or(Error::Invalid)?;
                    std::str::from_utf8(scalar).map_err(|_| Error::Invalid)?;
                    self.position += width;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pretty(bytes: &[u8], settings: &FormatterSettings) -> ResponseBody {
        pretty_body(
            &ResponseBody::from(bytes.to_vec()),
            settings,
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn fixtures_preserve_values_and_use_file_storage() {
        for input in [
            r#" {"z":[1,true,null,{},[]],"a":{"b":"hello"}} "#,
            r#""scalar""#,
            "false",
            "123.25",
            " [] ",
            "{}",
            r#"{"unicode":"é漢😀","escapes":"\\\"\/\b\f\n\r\t\u0000\uD83D\uDE00"}"#,
        ] {
            let output = pretty(input.as_bytes(), &FormatterSettings::default());
            assert!(output.is_file_backed(), "{input}");
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(input.as_bytes()).unwrap(),
                serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
            );
        }
    }

    #[test]
    fn indentation_order_and_token_spelling_are_preserved() {
        let settings = FormatterSettings {
            indent_size: 4,
            ..Default::default()
        };
        let input = br#"{"z":-0.000e+12345,"a":[123456789012345678901234567890,"\u0061"]}"#;
        let output = pretty(input, &settings);
        assert_eq!(
            output.as_ref(),
            b"{\n    \"z\": -0.000e+12345,\n    \"a\": [\n        123456789012345678901234567890,\n        \"\\u0061\"\n    ]\n}"
        );
        let tabs = pretty(
            br#"{"a":[{},[]]}"#,
            &FormatterSettings {
                hard_tabs: true,
                ..settings
            },
        );
        assert_eq!(tabs.as_ref(), b"{\n\t\"a\": [\n\t\t{},\n\t\t[]\n\t]\n}");
    }

    #[test]
    fn invalid_input_returns_the_same_shared_body() {
        for input in [
            &b""[..],
            b" ",
            b"true false",
            b"[1,]",
            b"{\"a\":1,}",
            b"{1:2}",
            b"{\"a\" 1}",
            b"[}",
            b"[1 2]",
            b"01",
            b"-",
            b"1.",
            b"1e+",
            b"NaN",
            b"\"unterminated",
            b"\"\\q\"",
            b"\"\\uD800\"",
            b"\"\\uDC00\"",
            b"\"\\uD800\\u0041\"",
            b"\"raw\nnewline\"",
            b"\"\xff\"",
            b"\"\xc0\xaf\"",
            b"\"\xed\xa0\x80\"",
            b"\"\xf4\x90\x80\x80\"",
            b"\xef\xbb\xbf{}",
        ] {
            let original = ResponseBody::from(input.to_vec());
            let result = pretty_body(
                &original,
                &FormatterSettings::default(),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(result.as_ptr(), original.as_ptr(), "{input:?}");
            assert_eq!(result.as_ref(), input);
        }
    }

    #[test]
    fn giant_string_and_number_use_bounded_writes() {
        // Build input on disk, not in a giant test String/Vec. The writer
        // below enforces the production token-copy bound as well.
        struct BoundedWriter {
            written: usize,
        }
        impl Write for BoundedWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                assert!(bytes.len() <= CHUNK);
                self.written += bytes.len();
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        for delimiter in [Some(b'"'), None] {
            let mut file = tempfile::tempfile().unwrap();
            if let Some(byte) = delimiter {
                file.write_all(&[byte]).unwrap();
            }
            let block = [b'1'; CHUNK];
            for _ in 0..640 {
                file.write_all(&block).unwrap();
            }
            if let Some(byte) = delimiter {
                file.write_all(&[byte]).unwrap();
            }
            // SAFETY: the completed anonymous fixture has a single owner.
            let input = unsafe { ResponseBody::file(file) }.unwrap();
            let mut writer = BoundedWriter { written: 0 };
            Formatter {
                input: &input,
                position: 0,
                next_check: 0,
                output: &mut writer,
                cancelled: &AtomicBool::new(false),
                indent: 2,
                indent_byte: b' ',
                stack: vec![State::RootValue],
            }
            .run()
            .unwrap();
            assert_eq!(writer.written, input.len());
            let result = pretty_body(
                &input,
                &FormatterSettings::default(),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert!(result.is_file_backed());
            assert_eq!(result.as_ref(), input.as_ref());
        }
    }

    #[test]
    fn deep_containers_do_not_use_the_call_stack() {
        let depth = 512;
        let input = format!("{}null{}", "[".repeat(depth), "]".repeat(depth));
        let output = pretty(input.as_bytes(), &FormatterSettings::default());
        assert!(output.is_file_backed());
        assert_eq!(
            output
                .iter()
                .copied()
                .filter(|byte| !byte.is_ascii_whitespace())
                .collect::<Vec<_>>(),
            input.as_bytes(),
        );
    }

    #[test]
    fn cancellation_before_and_during_output_is_reported() {
        let input = ResponseBody::from(b"{}".to_vec());
        assert!(
            pretty_body(
                &input,
                &FormatterSettings::default(),
                &AtomicBool::new(true),
            )
            .unwrap_err()
            .contains("cancelled")
        );

        struct CancelWriter<'a>(&'a AtomicBool);
        impl Write for CancelWriter<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.store(true, Ordering::Relaxed);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let cancelled = AtomicBool::new(false);
        let mut writer = CancelWriter(&cancelled);
        let bytes = vec![b'1'; CHUNK * 2];
        let result = Formatter {
            input: &bytes,
            position: 0,
            next_check: 0,
            output: &mut writer,
            cancelled: &cancelled,
            indent: 2,
            indent_byte: b' ',
            stack: vec![State::RootValue],
        }
        .run();
        assert!(matches!(result, Err(Error::Cancelled)));
    }
}
