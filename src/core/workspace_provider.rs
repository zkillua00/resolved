#![allow(dead_code)]

use std::{collections::HashMap, fmt, sync::Arc};

use thiserror::Error;

use super::{DatabaseStore, RequestTabs, Workspace, database::DatabaseError};

/// Stable identity for a source of workspace data.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum WorkspaceProviderId {
    Local(String),
    Upstream {
        upstream_id: String,
        workspace_id: String,
    },
}

impl fmt::Display for WorkspaceProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(workspace_id) => write!(formatter, "local:{workspace_id}"),
            Self::Upstream {
                upstream_id,
                workspace_id,
            } => write!(formatter, "upstream:{upstream_id}:{workspace_id}"),
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
    #[error("this workspace is read-only")]
    ReadOnly,
}

/// Storage boundary for collections, environments, snippets, and saved
/// requests. The UI owns a registry rather than a concrete SQLite store, so
/// collection call sites do not depend on workspace ownership.
///
/// Provider calls are intentionally side-effect free until they return `Ok`.
/// Network-backed implementations should be invoked from the app's network
/// runtime; local provider calls are short synchronous SQLite I/O.
pub trait WorkspaceProvider: Send + Sync {
    fn id(&self) -> WorkspaceProviderId;

    fn load_workspace(&self) -> Result<Workspace, WorkspaceProviderError>;

    fn load_request_tabs(&self) -> Result<RequestTabs, WorkspaceProviderError>;

    fn save_workspace(&self, workspace: &Workspace) -> Result<(), WorkspaceProviderError>;

    fn save_request_tabs(&self, request_tabs: &RequestTabs) -> Result<(), WorkspaceProviderError>;

    fn save_workspace_and_request_tabs(
        &self,
        workspace: &Workspace,
        request_tabs: &RequestTabs,
    ) -> Result<(), WorkspaceProviderError>;

    fn workspace_writable(&self) -> bool {
        true
    }

    fn request_tabs_writable(&self) -> bool {
        true
    }
}

#[derive(Clone, Debug)]
pub struct LocalWorkspaceProvider {
    database: DatabaseStore,
    workspace_id: String,
}

impl LocalWorkspaceProvider {
    pub fn new(database: DatabaseStore, workspace_id: impl Into<String>) -> Self {
        Self {
            database,
            workspace_id: workspace_id.into(),
        }
    }
}

impl WorkspaceProvider for LocalWorkspaceProvider {
    fn id(&self) -> WorkspaceProviderId {
        WorkspaceProviderId::Local(self.workspace_id.clone())
    }

    fn load_workspace(&self) -> Result<Workspace, WorkspaceProviderError> {
        self.database
            .load_workspace_for(&self.workspace_id)
            .map_err(WorkspaceProviderError::Local)
    }

    fn load_request_tabs(&self) -> Result<RequestTabs, WorkspaceProviderError> {
        self.database
            .load_request_tabs_for(&self.workspace_id)
            .map_err(WorkspaceProviderError::Local)
    }

    fn save_workspace(&self, workspace: &Workspace) -> Result<(), WorkspaceProviderError> {
        self.database
            .save_workspace_for(&self.workspace_id, workspace)
            .map_err(WorkspaceProviderError::Local)
    }

    fn save_request_tabs(&self, request_tabs: &RequestTabs) -> Result<(), WorkspaceProviderError> {
        self.database
            .save_request_tabs_for(&self.workspace_id, request_tabs)
            .map_err(WorkspaceProviderError::Local)
    }

    fn save_workspace_and_request_tabs(
        &self,
        workspace: &Workspace,
        request_tabs: &RequestTabs,
    ) -> Result<(), WorkspaceProviderError> {
        self.database
            .save_workspace_and_request_tabs_for(&self.workspace_id, workspace, request_tabs)
            .map_err(WorkspaceProviderError::Local)
    }
}

#[derive(Clone, Debug)]
pub struct RemoteWorkspaceProvider {
    id: WorkspaceProviderId,
    workspace: Workspace,
    database: DatabaseStore,
}

impl RemoteWorkspaceProvider {
    pub fn new(
        database: DatabaseStore,
        upstream_id: String,
        workspace_id: String,
        workspace: Workspace,
    ) -> Self {
        Self {
            id: WorkspaceProviderId::Upstream {
                upstream_id,
                workspace_id,
            },
            workspace,
            database,
        }
    }
}

impl WorkspaceProvider for RemoteWorkspaceProvider {
    fn id(&self) -> WorkspaceProviderId {
        self.id.clone()
    }

    fn load_workspace(&self) -> Result<Workspace, WorkspaceProviderError> {
        Ok(self.workspace.clone())
    }

    fn load_request_tabs(&self) -> Result<RequestTabs, WorkspaceProviderError> {
        let WorkspaceProviderId::Upstream {
            upstream_id,
            workspace_id,
        } = &self.id
        else {
            unreachable!("remote provider identity must be upstream")
        };
        self.database
            .load_upstream_request_tabs(upstream_id, workspace_id)
            .map_err(WorkspaceProviderError::Local)
    }

    fn save_workspace(&self, _: &Workspace) -> Result<(), WorkspaceProviderError> {
        Err(WorkspaceProviderError::ReadOnly)
    }

    fn save_request_tabs(&self, request_tabs: &RequestTabs) -> Result<(), WorkspaceProviderError> {
        let WorkspaceProviderId::Upstream {
            upstream_id,
            workspace_id,
        } = &self.id
        else {
            unreachable!("remote provider identity must be upstream")
        };
        self.database
            .save_upstream_request_tabs(upstream_id, workspace_id, request_tabs)
            .map_err(WorkspaceProviderError::Local)
    }

    fn save_workspace_and_request_tabs(
        &self,
        _: &Workspace,
        _: &RequestTabs,
    ) -> Result<(), WorkspaceProviderError> {
        Err(WorkspaceProviderError::ReadOnly)
    }

    fn workspace_writable(&self) -> bool {
        false
    }
}

/// Runtime registry for switchable workspace providers.
#[derive(Clone)]
pub struct WorkspaceProviderRegistry {
    providers: HashMap<WorkspaceProviderId, Arc<dyn WorkspaceProvider>>,
    active: WorkspaceProviderId,
}

impl WorkspaceProviderRegistry {
    pub fn local(database: DatabaseStore, workspace_id: impl Into<String>) -> Self {
        let provider: Arc<dyn WorkspaceProvider> =
            Arc::new(LocalWorkspaceProvider::new(database, workspace_id));
        let active = provider.id();
        let mut providers = HashMap::new();
        providers.insert(active.clone(), provider);
        Self { providers, active }
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

    pub fn provider(
        &self,
        provider_id: &WorkspaceProviderId,
    ) -> Result<&dyn WorkspaceProvider, WorkspaceProviderError> {
        self.providers
            .get(provider_id)
            .map(AsRef::as_ref)
            .ok_or_else(|| WorkspaceProviderError::NotRegistered(provider_id.clone()))
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

    pub fn remove_upstream(&mut self, upstream_id: &str) {
        self.providers.retain(|id, _| {
            !matches!(
                id,
                WorkspaceProviderId::Upstream {
                    upstream_id: candidate,
                    ..
                } if candidate == upstream_id
            )
        });
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

        fn load_request_tabs(&self) -> Result<RequestTabs, WorkspaceProviderError> {
            Ok(RequestTabs::default())
        }

        fn save_workspace(&self, workspace: &Workspace) -> Result<(), WorkspaceProviderError> {
            *self.workspace.lock().unwrap() = workspace.clone();
            Ok(())
        }

        fn save_request_tabs(&self, _: &RequestTabs) -> Result<(), WorkspaceProviderError> {
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
        let active_local_id = database.active_local_workspace_id().unwrap();
        let local_id = WorkspaceProviderId::Local(active_local_id.clone());
        let mut providers = WorkspaceProviderRegistry::local(database, active_local_id);
        let remote_id = WorkspaceProviderId::Upstream {
            upstream_id: "server-a".to_owned(),
            workspace_id: "workspace-a".to_owned(),
        };
        let mut remote_workspace = Workspace::default();
        remote_workspace.create_collection("Remote").unwrap();
        providers.register(Arc::new(MemoryProvider {
            id: remote_id.clone(),
            workspace: Mutex::new(remote_workspace),
        }));

        assert_eq!(providers.active_id(), &local_id);
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
        let active_local_id = database.active_local_workspace_id().unwrap();
        let local_id = WorkspaceProviderId::Local(active_local_id.clone());
        let mut providers = WorkspaceProviderRegistry::local(database, active_local_id);

        assert!(matches!(
            providers.switch(WorkspaceProviderId::Upstream {
                upstream_id: "missing".to_owned(),
                workspace_id: "missing".to_owned(),
            }),
            Err(WorkspaceProviderError::NotRegistered(_))
        ));
        assert_eq!(providers.active_id(), &local_id);
    }
}
