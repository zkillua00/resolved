//! Shared LSP ↔ editor coordinate helpers.
//!
//! gpui-component maps an LSP `Position` column as a Unicode *scalar* index when
//! it converts diagnostics and completion edits back into its Rope (not UTF-16),
//! so these helpers deliberately do not count UTF-16 code units.
//!
//! These were previously copy-pasted into each intelligence module with slightly
//! divergent implementations and comments. They live here so the editor contract
//! only ever has to be updated in one place.

use lsp_types::{Position, Range};

/// Convert a byte range in `source` into an LSP `Range` via `source_position`.
pub fn source_range(source: &str, start: usize, end: usize) -> Range {
    Range::new(source_position(source, start), source_position(source, end))
}

/// Convert a byte offset in `source` into an LSP `Position`, counting the column
/// in Unicode scalars to stay aligned with the editor's Rope contract.
pub fn source_position(source: &str, requested_offset: usize) -> Position {
    let offset = clipped_char_boundary(source, requested_offset);
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, line)| line)
        .chars()
        .count() as u32;
    Position::new(line, column)
}

/// Clamp `requested_offset` to `source.len()` and back it off to a char boundary
/// so `&source[..offset]` cannot panic.
pub fn clipped_char_boundary(source: &str, requested_offset: usize) -> usize {
    let mut offset = requested_offset.min(source.len());
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}
