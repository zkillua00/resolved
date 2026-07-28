#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExecutionStage {
    PreRequest,
    Request,
    PostResponse,
}

impl ExecutionStage {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::PreRequest => "Pre-script…",
            Self::Request => "Sending…",
            Self::PostResponse => "Post-script…",
        }
    }
}
