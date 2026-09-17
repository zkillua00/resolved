#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RequestPane {
    Params,
    PathVariables,
    Headers,
    Body,
    PreRequest,
    PostResponse,
    Cookies,
    Documentation,
}

impl RequestPane {
    pub(super) fn index(self) -> usize {
        match self {
            Self::Params => 0,
            Self::PathVariables => 1,
            Self::Headers => 2,
            Self::Body => 3,
            Self::PreRequest => 4,
            Self::PostResponse => 5,
            Self::Cookies => 6,
            Self::Documentation => 7,
        }
    }

    pub(super) fn from_index(index: usize) -> Self {
        match index {
            1 => Self::PathVariables,
            2 => Self::Headers,
            3 => Self::Body,
            4 => Self::PreRequest,
            5 => Self::PostResponse,
            6 => Self::Cookies,
            7 => Self::Documentation,
            _ => Self::Params,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_pane_indices_round_trip() {
        for pane in [
            RequestPane::Params,
            RequestPane::PathVariables,
            RequestPane::Headers,
            RequestPane::Body,
            RequestPane::PreRequest,
            RequestPane::PostResponse,
            RequestPane::Cookies,
            RequestPane::Documentation,
        ] {
            assert_eq!(RequestPane::from_index(pane.index()), pane);
        }
        assert_eq!(RequestPane::from_index(usize::MAX), RequestPane::Params);
    }
}
