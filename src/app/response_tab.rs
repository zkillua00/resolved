#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResponseTab {
    Body,
    Headers,
    Preview,
    Scripts,
}

impl ResponseTab {
    pub(super) fn index(self) -> usize {
        match self {
            Self::Body => 0,
            Self::Headers => 1,
            Self::Preview => 2,
            Self::Scripts => 3,
        }
    }

    pub(super) fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Headers,
            2 => Self::Preview,
            3 => Self::Scripts,
            _ => Self::Body,
        }
    }
}
