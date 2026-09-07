#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RequestPane {
    Params,
    Headers,
    Body,
    PreRequest,
    PostResponse,
    Cookies,
}

impl RequestPane {
    pub(super) fn index(self) -> usize {
        match self {
            Self::Params => 0,
            Self::Headers => 1,
            Self::Body => 2,
            Self::PreRequest => 3,
            Self::PostResponse => 4,
            Self::Cookies => 5,
        }
    }

    pub(super) fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Headers,
            2 => Self::Body,
            3 => Self::PreRequest,
            4 => Self::PostResponse,
            5 => Self::Cookies,
            _ => Self::Params,
        }
    }
}
