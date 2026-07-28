#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RequestTab {
    Headers,
    Body,
    PreRequest,
    PostResponse,
}

impl RequestTab {
    pub(super) fn index(self) -> usize {
        match self {
            Self::Headers => 0,
            Self::Body => 1,
            Self::PreRequest => 2,
            Self::PostResponse => 3,
        }
    }

    pub(super) fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Body,
            2 => Self::PreRequest,
            3 => Self::PostResponse,
            _ => Self::Headers,
        }
    }
}
