use super::*;

impl ApiTester {
    pub(super) fn persist_history(&mut self) {
        if !self.history_writable {
            self.history_warning.get_or_insert_with(|| {
                "History is read-only because it could not be loaded safely.".to_owned()
            });
            return;
        }
        self.history_warning = self
            .database_store
            .save_history(&self.history)
            .err()
            .map(|error| format!("History could not be saved: {error}"));
    }

    pub(super) fn commit_workspace(&mut self, candidate: Workspace) -> Result<(), String> {
        if !self.workspace_writable {
            return Err("Database is read-only for this session.".to_owned());
        }
        match self.workspace_providers.active().save_workspace(&candidate) {
            Ok(()) => {
                self.workspace = candidate;
                self.workspace_warning = None;
                Ok(())
            }
            Err(error) => {
                let message = format!("Workspace could not be saved: {error}");
                self.workspace_warning = Some(message.clone());
                Err(message)
            }
        }
    }

    pub(super) fn commit_workspace_and_request_tabs(
        &mut self,
        candidate_workspace: Workspace,
        candidate_request_tabs: RequestTabs,
    ) -> Result<(), String> {
        if !self.workspace_writable {
            return Err("Database is read-only for this session.".to_owned());
        }
        if !self.request_tabs_writable {
            self.commit_workspace(candidate_workspace)?;
            self.request_tabs = candidate_request_tabs;
            return Ok(());
        }
        match self
            .workspace_providers
            .active()
            .save_workspace_and_request_tabs(&candidate_workspace, &candidate_request_tabs)
        {
            Ok(()) => {
                self.workspace = candidate_workspace;
                self.request_tabs = candidate_request_tabs;
                self.last_persisted_request_tabs = self.request_tabs.clone();
                self.workspace_warning = None;
                self.request_tabs_warning = None;
                self.request_tabs_persist_task = None;
                Ok(())
            }
            Err(error) => {
                let message = format!("Workspace and request tabs could not be saved: {error}");
                self.workspace_warning = Some(message.clone());
                self.request_tabs_warning = Some(message.clone());
                Err(message)
            }
        }
    }
}
