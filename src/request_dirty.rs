/// Independently tracked portions of the persisted request editor state.
///
/// Keeping these as bits lets input callbacks update only the part they own
/// without rebuilding a complete `RequestTemplate` on every keystroke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestDirtyPart {
    Method,
    Url,
    Params,
    Headers,
    RawBody,
    BodyMode,
    RawBodyLanguage,
    BodyFields,
    PreScript,
    PostScript,
}

impl RequestDirtyPart {
    const fn mask(self) -> u16 {
        1 << self as u16
    }
}

/// Incremental dirty state for the request editor.
///
/// `set` reports only aggregate clean/dirty transitions. That is the signal
/// needed by consumers such as the title bar's modified indicator; changing a
/// second bit while the request is already dirty does not require repainting
/// that indicator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RequestDirtyState {
    bits: u16,
    hydration_depth: u16,
}

impl RequestDirtyState {
    pub(crate) const fn any(self) -> bool {
        self.bits != 0
    }

    #[cfg(test)]
    const fn contains(self, part: RequestDirtyPart) -> bool {
        self.bits & part.mask() != 0
    }

    /// Set or clear one dirty part.
    ///
    /// Returns `true` only when the aggregate state changes between clean and
    /// dirty. Updates emitted by controls while a template is being hydrated
    /// are intentionally ignored.
    pub(crate) fn set(&mut self, part: RequestDirtyPart, dirty: bool) -> bool {
        if self.is_hydrating() {
            return false;
        }

        let was_dirty = self.any();
        if dirty {
            self.bits |= part.mask();
        } else {
            self.bits &= !part.mask();
        }
        was_dirty != self.any()
    }

    /// Clear all tracked parts and report an aggregate dirty-to-clean change.
    pub(crate) fn clear(&mut self) -> bool {
        let was_dirty = self.any();
        self.bits = 0;
        was_dirty
    }

    /// Suppress input-change bookkeeping during programmatic template loads.
    ///
    /// Calls may be nested so helper methods can safely participate in a
    /// larger hydration operation.
    pub(crate) fn begin_hydration(&mut self) {
        self.hydration_depth = self
            .hydration_depth
            .checked_add(1)
            .expect("request hydration nesting overflowed");
    }

    /// Finish a hydration scope.
    ///
    /// The outermost scope establishes a new clean baseline, so any dirty bits
    /// that existed before hydration are cleared. Returns `true` when that
    /// changes the aggregate state from dirty to clean.
    pub(crate) fn end_hydration(&mut self) -> bool {
        assert!(
            self.hydration_depth > 0,
            "request hydration ended without being started"
        );
        self.hydration_depth -= 1;
        if self.is_hydrating() {
            false
        } else {
            self.clear()
        }
    }

    pub(crate) const fn is_hydrating(self) -> bool {
        self.hydration_depth != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_are_independent_and_reverting_the_last_part_becomes_clean() {
        let mut state = RequestDirtyState::default();

        assert!(state.set(RequestDirtyPart::Url, true));
        assert!(state.any());
        assert!(state.contains(RequestDirtyPart::Url));

        assert!(!state.set(RequestDirtyPart::Headers, true));
        assert!(state.contains(RequestDirtyPart::Url));
        assert!(state.contains(RequestDirtyPart::Headers));

        assert!(!state.set(RequestDirtyPart::Url, false));
        assert!(state.any());
        assert!(!state.contains(RequestDirtyPart::Url));
        assert!(state.contains(RequestDirtyPart::Headers));

        assert!(state.set(RequestDirtyPart::Headers, false));
        assert!(!state.any());
    }

    #[test]
    fn every_persisted_editor_part_has_a_distinct_bit() {
        let parts = [
            RequestDirtyPart::Method,
            RequestDirtyPart::Url,
            RequestDirtyPart::Params,
            RequestDirtyPart::Headers,
            RequestDirtyPart::RawBody,
            RequestDirtyPart::BodyMode,
            RequestDirtyPart::RawBodyLanguage,
            RequestDirtyPart::BodyFields,
            RequestDirtyPart::PreScript,
            RequestDirtyPart::PostScript,
        ];
        let combined = parts.iter().fold(0_u16, |bits, part| bits | part.mask());

        assert_eq!(combined.count_ones(), parts.len() as u32);
    }

    #[test]
    fn clear_resets_all_parts_and_reports_only_an_aggregate_transition() {
        let mut state = RequestDirtyState::default();
        assert!(!state.clear());

        state.set(RequestDirtyPart::BodyMode, true);
        state.set(RequestDirtyPart::PreScript, true);
        assert!(state.clear());
        assert!(!state.any());
        assert!(!state.contains(RequestDirtyPart::BodyMode));
        assert!(!state.contains(RequestDirtyPart::PreScript));
        assert!(!state.clear());
    }

    #[test]
    fn hydration_ignores_programmatic_change_events_and_establishes_clean_state() {
        let mut state = RequestDirtyState::default();
        state.set(RequestDirtyPart::RawBody, true);

        state.begin_hydration();
        assert!(state.is_hydrating());
        assert!(!state.set(RequestDirtyPart::Method, true));
        assert!(!state.set(RequestDirtyPart::RawBody, false));
        assert!(state.contains(RequestDirtyPart::RawBody));
        assert!(!state.contains(RequestDirtyPart::Method));

        assert!(state.end_hydration());
        assert!(!state.is_hydrating());
        assert!(!state.any());

        assert!(state.set(RequestDirtyPart::PostScript, true));
        assert!(state.contains(RequestDirtyPart::PostScript));
    }

    #[test]
    fn nested_hydration_clears_only_when_the_outer_scope_finishes() {
        let mut state = RequestDirtyState::default();
        state.set(RequestDirtyPart::BodyFields, true);

        state.begin_hydration();
        state.begin_hydration();
        assert!(!state.end_hydration());
        assert!(state.is_hydrating());
        assert!(state.any());
        assert!(!state.set(RequestDirtyPart::Url, true));

        assert!(state.end_hydration());
        assert!(!state.is_hydrating());
        assert!(!state.any());
    }

    #[test]
    #[should_panic(expected = "request hydration ended without being started")]
    fn hydration_scopes_must_be_balanced() {
        RequestDirtyState::default().end_hydration();
    }
}
