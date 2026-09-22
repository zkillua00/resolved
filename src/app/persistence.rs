use super::*;

/// UI bookkeeping only. A queued operation is not a durable success.
#[derive(Default)]
pub(super) struct PersistenceIoState {
    pub(super) history_generation: u64,
    pub(super) history_pending: bool,
    pub(super) history_failed: bool,
    pub(super) history_clear_pending: bool,
    pub(super) history_save_deferred: bool,
    pub(super) snippets_pending: bool,
}

fn enqueue_snippets_save(
    database: DatabaseStore,
    candidate: Vec<Snippet>,
) -> crate::io::IoTask<Result<Vec<Snippet>, String>> {
    crate::io::run(move || {
        database
            .save_snippets(&candidate)
            .map_err(|error| format!("Snippets could not be saved: {error}"))?;
        Ok(candidate)
    })
}

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

    pub(super) fn persist_history(&mut self, cx: &mut Context<Self>) {
        if self.persistence_io.history_clear_pending {
            self.persistence_io.history_save_deferred = true;
            return;
        }
        if !self.history_writable {
            self.history_warning.get_or_insert_with(|| {
                "History is read-only because it could not be loaded safely.".to_owned()
            });
            return;
        }
        self.persistence_io.history_generation =
            self.persistence_io.history_generation.wrapping_add(1);
        let generation = self.persistence_io.history_generation;
        self.persistence_io.history_pending = true;
        let database = self.database_store.clone();
        let history = self.history.clone();
        // Enqueue before spawning the observer: every snapshot enters the
        // shared FIFO in UI event order, even if observers poll out of order.
        let save = crate::io::run(move || database.save_history(&history));
        cx.spawn(async move |this, cx| {
            let result = save
                .await
                .map_err(|error| format!("History I/O failed: {error}"))
                .and_then(|result| {
                    result.map_err(|error| format!("History could not be saved: {error}"))
                });
            let _ = this.update(cx, |this, cx| {
                if this.persistence_io.history_generation != generation {
                    return;
                }
                this.persistence_io.history_pending = false;
                this.persistence_io.history_failed = result.is_err();
                this.history_warning = result.err();
                cx.notify();
            });
        })
        .detach();
    }

    /// Isolated executions contribute entries, never a stale copy of the entire
    /// history table (saving that copy would prune another run's entries).
    pub(super) fn persist_execution_history(&mut self, cx: &mut Context<Self>) {
        let Some(owner) = self.mcp_execution_owner.clone() else {
            self.persist_history(cx);
            return;
        };
        let entries = self.history.entries().to_vec();
        self.mcp_execution_history_ids
            .extend(entries.iter().map(|entry| entry.id.clone()));
        self.history.clear();
        let _ = owner.update(cx, |owner, cx| {
            for entry in entries.into_iter().rev() {
                owner.history.push(entry);
            }
            owner.persist_history(cx);
            cx.notify();
        });
    }

    pub(super) fn begin_snippets_save(
        &mut self,
        candidate: Vec<Snippet>,
    ) -> Result<crate::io::IoTask<Result<Vec<Snippet>, String>>, String> {
        if self.persistence_io.snippets_pending {
            return Err("A snippet save is still pending. Try again when it completes.".to_owned());
        }
        self.persistence_io.snippets_pending = true;
        Ok(enqueue_snippets_save(
            self.database_store.clone(),
            candidate,
        ))
    }

    /// All snippet writers (including control requests) use this completion
    /// step. Failed writes leave the live list and editor baseline untouched.
    pub(super) fn finish_snippets_save(
        &mut self,
        result: Result<Vec<Snippet>, String>,
    ) -> Result<(), String> {
        self.persistence_io.snippets_pending = false;
        self.snippets = result?;
        self.invalidate_snippet_list_cache();
        Ok(())
    }

    /// Called again after the close barrier. The barrier only drains worker
    /// jobs; foreground completion handlers must also have recorded success.
    pub(crate) fn local_persistence_ready_to_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.has_pending_execution(cx) || self.gateway.pending > 0 {
            self.request_notice = Some(
                "Finish or cancel running requests before closing; their history is not finalized."
                    .into(),
            );
            cx.notify();
            return false;
        }
        if self.persistence_io.history_pending || self.persistence_io.snippets_pending {
            self.request_notice = Some(
                "Local changes are still being saved. Wait for completion before closing.".into(),
            );
            cx.notify();
            return false;
        }
        if self.persistence_io.history_failed {
            self.request_notice = Some(
                "History could not be saved. The window was kept open; try closing again to retry."
                    .into(),
            );
            self.persist_history(cx);
            cx.notify();
            return false;
        }
        true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn snippet_save_completion_means_the_candidate_is_durable() {
        let directory = tempfile::tempdir().unwrap();
        let database = DatabaseStore::new(directory.path().join("snippets.sqlite3"));
        let setup = database.clone();
        crate::io::run(move || setup.initialize())
            .await
            .unwrap()
            .unwrap();
        let first = Snippet::new("First", SnippetCategory::PreRequest, SnippetKind::Plain).unwrap();
        let second =
            Snippet::new("Second", SnippetCategory::PreRequest, SnippetKind::Plain).unwrap();

        // Eager FIFO enqueueing must not depend on the order futures are polled.
        let first_save = enqueue_snippets_save(database.clone(), vec![first]);
        let second_save = enqueue_snippets_save(database.clone(), vec![second.clone()]);
        let saved = second_save.await.unwrap().unwrap();
        first_save.await.unwrap().unwrap();
        assert_eq!(saved[0].id, second.id);
        let durable = crate::io::run(move || database.load_snippets())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(durable.len(), 1);
        assert_eq!(durable[0].id, second.id);
    }

    #[tokio::test]
    async fn snippet_save_failure_does_not_return_a_successful_candidate() {
        let directory = tempfile::tempdir().unwrap();
        // A directory cannot be opened as a SQLite database.
        let database = DatabaseStore::new(directory.path().to_path_buf());
        let snippet =
            Snippet::new("Unsaved", SnippetCategory::PreRequest, SnippetKind::Plain).unwrap();
        let result = enqueue_snippets_save(database, vec![snippet])
            .await
            .unwrap();
        assert!(result.unwrap_err().contains("Snippets could not be saved"));
    }
}
