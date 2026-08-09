#![allow(dead_code)]

use std::{collections::HashMap, fmt, sync::Arc};

use thiserror::Error;

use super::{DatabaseStore, RequestTabs, Workspace, database::DatabaseError};

/// Stable identity for a source of workspace data.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum WorkspaceProviderId {
    Local,
    Upstream(String),
}

impl fmt::Display for WorkspaceProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => formatter.write_str("local"),
            Self::Upstream(id) => write!(formatter, "upstream:{id}"),
        }
    }
}

#[derive(Debug, Error)]
pub enum WorkspaceProviderError {
    #[error("the workspace provider {0} is not registered")]
    NotRegistered(WorkspaceProviderId),
    #[error("local workspace storage failed: {0}")]
    Local(#[from] DatabaseError),
    #[error("workspace provider failed: {0}")]
    Provider(String),
}

/// Storage boundary for collections, environments, snippets, and saved
/// requests. The UI owns a registry rather than a concrete SQLite store, so a
/// future upstream workspace can implement this contract without changing
/// collection call sites.
///
/// Provider calls are intentionally side-effect free until they return `Ok`.
/// Network-backed implementations should be invoked from the app's network
/// runtime; the current local implementation is short synchronous SQLite I/O.
pub trait WorkspaceProvider: Send + Sync {
    fn id(&self) -> WorkspaceProviderId;

    fn load_workspace(&self) -> Result<Workspace, WorkspaceProviderError>;

    fn save_workspace(&self, workspace: &Workspace) -> Result<(), WorkspaceProviderError>;

    fn save_workspace_and_request_tabs(
        &self,
        workspace: &Workspace,
        request_tabs: &RequestTabs,
    ) -> Result<(), WorkspaceProviderError>;
}

#[derive(Clone, Debug)]
pub struct LocalWorkspaceProvider {
    database: DatabaseStore,
}

impl LocalWorkspaceProvider {
    pub fn new(database: DatabaseStore) -> Self {
        Self { database }
    }
}

impl WorkspaceProvider for LocalWorkspaceProvider {
    fn id(&self) -> WorkspaceProviderId {
        WorkspaceProviderId::Local
    }

    fn load_workspace(&self) -> Result<Workspace, WorkspaceProviderError> {
        self.database
            .load_workspace()
            .map_err(WorkspaceProviderError::Local)
    }

    fn save_workspace(&self, workspace: &Workspace) -> Result<(), WorkspaceProviderError> {
        self.database
            .save_workspace(workspace)
            .map_err(WorkspaceProviderError::Local)
    }

    fn save_workspace_and_request_tabs(
        &self,
        workspace: &Workspace,
        request_tabs: &RequestTabs,
    ) -> Result<(), WorkspaceProviderError> {
        self.database
            .save_workspace_and_request_tabs(workspace, request_tabs)
            .map_err(WorkspaceProviderError::Local)
    }
}

/// Runtime registry for switchable workspace providers.
///
/// Only the local provider is installed in this stage. Adding a server selects
/// an upstream connection, but it does not move or expose the local workspace;
/// the later workspace protocol can register its remote provider here.
#[derive(Clone)]
pub struct WorkspaceProviderRegistry {
    providers: HashMap<WorkspaceProviderId, Arc<dyn WorkspaceProvider>>,
    active: WorkspaceProviderId,
}

impl WorkspaceProviderRegistry {
    pub fn local(database: DatabaseStore) -> Self {
        let provider: Arc<dyn WorkspaceProvider> = Arc::new(LocalWorkspaceProvider::new(database));
        let mut providers = HashMap::new();
        providers.insert(WorkspaceProviderId::Local, provider);
        Self {
            providers,
            active: WorkspaceProviderId::Local,
        }
    }

    pub fn active_id(&self) -> &WorkspaceProviderId {
        &self.active
    }

    pub fn active(&self) -> &dyn WorkspaceProvider {
        self.providers
            .get(&self.active)
            .expect("active workspace provider must remain registered")
            .as_ref()
    }

    pub fn register(&mut self, provider: Arc<dyn WorkspaceProvider>) {
        self.providers.insert(provider.id(), provider);
    }

    pub fn switch(
        &mut self,
        provider_id: WorkspaceProviderId,
    ) -> Result<(), WorkspaceProviderError> {
        if !self.providers.contains_key(&provider_id) {
            return Err(WorkspaceProviderError::NotRegistered(provider_id));
        }
        self.active = provider_id;
        Ok(())
    }

    pub fn contains(&self, provider_id: &WorkspaceProviderId) -> bool {
        self.providers.contains_key(provider_id)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct MemoryProvider {
        id: WorkspaceProviderId,
        workspace: Mutex<Workspace>,
    }

    impl WorkspaceProvider for MemoryProvider {
        fn id(&self) -> WorkspaceProviderId {
            self.id.clone()
        }

        fn load_workspace(&self) -> Result<Workspace, WorkspaceProviderError> {
            Ok(self.workspace.lock().unwrap().clone())
        }

        fn save_workspace(&self, workspace: &Workspace) -> Result<(), WorkspaceProviderError> {
            *self.workspace.lock().unwrap() = workspace.clone();
            Ok(())
        }

        fn save_workspace_and_request_tabs(
            &self,
            workspace: &Workspace,
            _: &RequestTabs,
        ) -> Result<(), WorkspaceProviderError> {
            self.save_workspace(workspace)
        }
    }

    #[test]
    fn registry_switches_between_independent_provider_instances() {
        let directory = tempfile::tempdir().unwrap();
        let database = DatabaseStore::new(directory.path().join("workspace.sqlite3"));
        database.initialize().unwrap();
        let mut providers = WorkspaceProviderRegistry::local(database);
        let remote_id = WorkspaceProviderId::Upstream("server-a".to_owned());
        let mut remote_workspace = Workspace::default();
        remote_workspace.create_collection("Remote").unwrap();
        providers.register(Arc::new(MemoryProvider {
            id: remote_id.clone(),
            workspace: Mutex::new(remote_workspace),
        }));

        assert_eq!(providers.active_id(), &WorkspaceProviderId::Local);
        assert!(
            providers
                .active()
                .load_workspace()
                .unwrap()
                .collections
                .is_empty()
        );
        providers.switch(remote_id.clone()).unwrap();
        assert_eq!(providers.active_id(), &remote_id);
        assert_eq!(
            providers.active().load_workspace().unwrap().collections[0].name,
            "Remote"
        );
    }

    #[test]
    fn registry_rejects_unknown_provider_without_changing_selection() {
        let directory = tempfile::tempdir().unwrap();
        let database = DatabaseStore::new(directory.path().join("workspace.sqlite3"));
        database.initialize().unwrap();
        let mut providers = WorkspaceProviderRegistry::local(database);

        assert!(matches!(
            providers.switch(WorkspaceProviderId::Upstream("missing".to_owned())),
            Err(WorkspaceProviderError::NotRegistered(_))
        ));
        assert_eq!(providers.active_id(), &WorkspaceProviderId::Local);
    }
}
