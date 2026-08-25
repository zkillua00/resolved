use super::*;

impl ApiTester {
    /// Replace the active workspace and bump the derived-cache version so the
    /// collections sidebar doesn't render a stale folder index.
    pub(super) fn replace_workspace(&mut self, workspace: Workspace) {
        self.workspace = workspace;
        self.workspace_version = self.workspace_version.wrapping_add(1);
        // Keep the script editors' saved-request model in sync with the active
        // workspace so completion/hover tend to reflect new, renamed, moved or
        // deleted collections/requests immediately — and the stale-reference
        // diagnostic never fires on a reference the runtime can still resolve
        // (the runtime builds a fresh catalog, so a stale editor catalog was
        // the drift). The full refresh_variable_intelligence additionally
        // re-runs editor diagnostics and pushes checked declarations.
        let namespace = crate::core::RequestNamespaceCatalog::from_workspace(&self.workspace);
        *self.script_request_namespace.borrow_mut() = namespace;
    }

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

    pub(super) fn persist_snippets(&mut self) -> Result<(), String> {
        self.database_store
            .save_snippets(&self.snippets)
            .map_err(|error| format!("Snippets could not be saved: {error}"))
    }

    pub(super) fn commit_workspace(&mut self, candidate: Workspace) -> Result<(), String> {
        if !self.workspace_writable {
            return Err("This workspace is read-only.".to_owned());
        }
        match self.workspace_providers.active().save_workspace(&candidate) {
            Ok(()) => {
                self.replace_workspace(candidate);
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
            return Err("This workspace is read-only.".to_owned());
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
                self.replace_workspace(candidate_workspace);
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
