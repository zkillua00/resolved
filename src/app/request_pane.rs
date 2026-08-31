#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RequestPane {
    Params,
    Headers,
    Body,
    PreRequest,
    PostResponse,
}

impl RequestPane {
    pub(super) fn index(self) -> usize {
        match self {
            Self::Params => 0,
            Self::Headers => 1,
            Self::Body => 2,
            Self::PreRequest => 3,
            Self::PostResponse => 4,
        }
    }

    pub(super) fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Headers,
            2 => Self::Body,
            3 => Self::PreRequest,
            4 => Self::PostResponse,
            _ => Self::Params,
        }
    }
}
