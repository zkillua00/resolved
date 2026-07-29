use std::ops::Range;

use gpui::{Entity, EntityId, Pixels, Point};
use gpui_component::input::InputState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TemplateVariableAction {
    Create,
    Update {
        variable_id: String,
        enable_on_save: bool,
    },
}

#[derive(Clone)]
pub(super) struct TemplateVariablePopover {
    pub(super) source_input: Entity<InputState>,
    pub(super) source_input_id: EntityId,
    pub(super) source_range: Range<usize>,
    pub(super) name: String,
    pub(super) expected_environment_id: Option<String>,
    pub(super) environment_name: Option<String>,
    pub(super) action: TemplateVariableAction,
    pub(super) secret: bool,
    pub(super) value: Entity<InputState>,
    pub(super) position: Point<Pixels>,
    pub(super) error: Option<String>,
}
