use gpui::{Entity, Pixels, Point};
use gpui_component::input::InputState;

#[derive(Clone)]
pub(super) enum TemplateVariableAction {
    Create,
    Enable { variable_id: String },
}

#[derive(Clone)]
pub(super) struct TemplateVariablePopover {
    pub(super) name: String,
    pub(super) expected_environment_id: Option<String>,
    pub(super) environment_name: Option<String>,
    pub(super) action: TemplateVariableAction,
    pub(super) value: Entity<InputState>,
    pub(super) position: Point<Pixels>,
    pub(super) error: Option<String>,
}
