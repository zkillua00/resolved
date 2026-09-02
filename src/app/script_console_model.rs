use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScriptConsoleTone {
    Neutral,
    Info,
    Warning,
    Danger,
    Success,
    Debug,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScriptConsoleValue {
    pub(super) kind: String,
    pub(super) preview: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScriptConsoleRow {
    pub(super) label: String,
    pub(super) message: String,
    pub(super) detail: Option<String>,
    pub(super) copy_value: String,
    pub(super) tone: ScriptConsoleTone,
    pub(super) values: Vec<ScriptConsoleValue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScriptConsoleSection {
    pub(super) key: String,
    pub(super) title: String,
    pub(super) duration: Option<Duration>,
    pub(super) rows: Vec<ScriptConsoleRow>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ScriptConsoleModel {
    pub(super) sections: Vec<ScriptConsoleSection>,
}
