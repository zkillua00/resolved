use super::*;

#[derive(Clone)]
pub(in crate::app) struct RequestTabRuntime {
    pub(in crate::app) request_pane: RequestPane,
    pub(in crate::app) response_tab: ResponseTab,
    pub(in crate::app) pretty_body: bool,
    pub(in crate::app) response: Option<ResponseData>,
    pub(in crate::app) request_error: Option<String>,
    pub(in crate::app) script_diagnostic: Option<ScriptDiagnostic>,
    pub(in crate::app) pre_script_report: Option<ScriptReport>,
    pub(in crate::app) post_script_report: Option<ScriptReport>,
    pub(in crate::app) preview_error: Option<String>,
    pub(in crate::app) copied: bool,
    pub(in crate::app) request_notice: Option<String>,
}

impl Default for RequestTabRuntime {
    fn default() -> Self {
        Self {
            request_pane: RequestPane::Headers,
            response_tab: ResponseTab::Body,
            pretty_body: true,
            response: None,
            request_error: None,
            script_diagnostic: None,
            pre_script_report: None,
            post_script_report: None,
            preview_error: None,
            copied: false,
            request_notice: None,
        }
    }
}
