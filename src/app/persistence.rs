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
        match self.database_store.save_workspace(&candidate) {
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
}
