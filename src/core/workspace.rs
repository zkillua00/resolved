use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    snippet::{Snippet, SnippetCategory, SnippetValidationError},
    template::RequestTemplate,
};

pub const WORKSPACE_FILE_VERSION: u32 = 1;

#[cfg(test)]
use std::fs::{self, File};
#[cfg(test)]
use std::io::{self, Write};
#[cfg(test)]
use std::path::{Path, PathBuf};

static NEXT_WORKSPACE_ID: AtomicU64 = AtomicU64::new(0);

fn enabled_by_default() -> bool {
    true
}

/// Persisted pre- and post-request source.
///
/// The script runtime owns execution semantics. Keeping the source alongside a
/// saved request makes collections self-contained, while serde defaults keep
/// files written before scripting was added readable.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestScripts {
    #[serde(default)]
    pub pre_request: String,
    #[serde(default)]
    pub post_response: String,
}

impl RequestScripts {
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.pre_request.trim().is_empty() && self.post_response.trim().is_empty()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Workspace {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<ResourceCreator>,
    #[serde(default)]
    pub collections: Vec<Collection>,
    #[serde(default)]
    pub environments: Vec<Environment>,
    #[serde(default)]
    pub active_environment_id: Option<String>,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceCreator {
    pub id: String,
    pub email: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Collection {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<ResourceCreator>,
    #[serde(default)]
    pub folders: Vec<CollectionFolder>,
    #[serde(default)]
    pub requests: Vec<SavedRequest>,
}

impl Collection {
    pub fn new(name: impl Into<String>) -> Result<Self, WorkspaceMutationError> {
        Ok(Self {
            id: new_id("collection"),
            name: checked_name("collection", name.into())?,
            created_by: None,
            folders: Vec::new(),
            requests: Vec::new(),
        })
    }

    pub fn folder(&self, id: &str) -> Option<&CollectionFolder> {
        self.folders.iter().find(|folder| folder.id == id)
    }

    pub fn folder_path_ids(&self, id: &str) -> Result<Vec<String>, WorkspaceMutationError> {
        self.ensure_folder_exists(Some(id))?;
        let mut path = Vec::new();
        let mut current_id = Some(id);
        let mut seen = HashSet::new();

        while let Some(folder_id) = current_id {
            if !seen.insert(folder_id) {
                return Err(WorkspaceMutationError::FolderCycle {
                    folder_id: id.to_owned(),
                });
            }
            let folder =
                self.folder(folder_id)
                    .ok_or_else(|| WorkspaceMutationError::NotFound {
                        kind: "folder",
                        id: folder_id.to_owned(),
                    })?;
            path.push(folder.id.clone());
            current_id = folder.parent_folder_id.as_deref();
        }
        path.reverse();
        Ok(path)
    }

    fn folder_mut(&mut self, id: &str) -> Result<&mut CollectionFolder, WorkspaceMutationError> {
        self.folders
            .iter_mut()
            .find(|folder| folder.id == id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "folder",
                id: id.to_owned(),
            })
    }

    fn ensure_folder_exists(&self, id: Option<&str>) -> Result<(), WorkspaceMutationError> {
        if let Some(id) = id
            && self.folder(id).is_none()
        {
            return Err(WorkspaceMutationError::NotFound {
                kind: "folder",
                id: id.to_owned(),
            });
        }
        Ok(())
    }

    fn folder_descendant_ids(&self, id: &str) -> HashSet<String> {
        let mut descendant_ids = HashSet::from([id.to_owned()]);
        loop {
            let previous_len = descendant_ids.len();
            for folder in &self.folders {
                if folder
                    .parent_folder_id
                    .as_deref()
                    .is_some_and(|parent_id| descendant_ids.contains(parent_id))
                {
                    descendant_ids.insert(folder.id.clone());
                }
            }
            if descendant_ids.len() == previous_len {
                return descendant_ids;
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectionFolder {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<ResourceCreator>,
    #[serde(default)]
    pub parent_folder_id: Option<String>,
}

impl CollectionFolder {
    pub fn new(
        name: impl Into<String>,
        parent_folder_id: Option<String>,
    ) -> Result<Self, WorkspaceMutationError> {
        Ok(Self {
            id: new_id("folder"),
            name: checked_name("folder", name.into())?,
            created_by: None,
            parent_folder_id,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedRequest {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<ResourceCreator>,
    #[serde(default)]
    pub folder_id: Option<String>,
    pub definition: RequestTemplate,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SavedRequest {
    pub fn new(
        name: impl Into<String>,
        definition: RequestTemplate,
    ) -> Result<Self, WorkspaceMutationError> {
        let now = Utc::now();
        Ok(Self {
            id: new_id("request"),
            name: checked_name("request", name.into())?,
            created_by: None,
            folder_id: None,
            definition,
            created_at: now,
            updated_at: now,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<ResourceCreator>,
    #[serde(default)]
    pub variables: Vec<EnvironmentVariable>,
}

impl Environment {
    pub fn new(name: impl Into<String>) -> Result<Self, WorkspaceMutationError> {
        Ok(Self {
            id: new_id("environment"),
            name: checked_name("environment", name.into())?,
            created_by: None,
            variables: Vec::new(),
        })
    }

    #[allow(dead_code)]
    pub fn variable(&self, id: &str) -> Option<&EnvironmentVariable> {
        self.variables.iter().find(|variable| variable.id == id)
    }

    pub fn add_variable(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
        enabled: bool,
        secret: bool,
    ) -> Result<String, WorkspaceMutationError> {
        let key = checked_variable_key(key.into())?;
        self.ensure_variable_key_available(&key, None)?;
        let variable = EnvironmentVariable {
            id: new_id("variable"),
            key,
            value: value.into(),
            enabled,
            secret,
            created_by: None,
        };
        let id = variable.id.clone();
        self.variables.push(variable);
        Ok(id)
    }

    pub fn update_variable(
        &mut self,
        id: &str,
        key: impl Into<String>,
        value: impl Into<String>,
        enabled: bool,
        secret: bool,
    ) -> Result<(), WorkspaceMutationError> {
        let key = checked_variable_key(key.into())?;
        self.ensure_variable_key_available(&key, Some(id))?;
        let variable = self
            .variables
            .iter_mut()
            .find(|variable| variable.id == id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "variable",
                id: id.to_owned(),
            })?;
        variable.key = key;
        variable.value = value.into();
        variable.enabled = enabled;
        variable.secret = secret;
        Ok(())
    }

    pub fn remove_variable(
        &mut self,
        id: &str,
    ) -> Result<EnvironmentVariable, WorkspaceMutationError> {
        let index = self
            .variables
            .iter()
            .position(|variable| variable.id == id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "variable",
                id: id.to_owned(),
            })?;
        Ok(self.variables.remove(index))
    }

    fn ensure_variable_key_available(
        &self,
        key: &str,
        except_id: Option<&str>,
    ) -> Result<(), WorkspaceMutationError> {
        if self
            .variables
            .iter()
            .any(|variable| variable.id != except_id.unwrap_or_default() && variable.key == key)
        {
            return Err(WorkspaceMutationError::DuplicateVariableKey {
                environment_id: self.id.clone(),
                key: key.to_owned(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvironmentVariable {
    pub id: String,
    pub key: String,
    pub value: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<ResourceCreator>,
}

impl Workspace {
    pub fn validate(&self) -> Result<(), WorkspaceValidationError> {
        let mut collection_ids = HashSet::new();
        let mut folder_ids = HashSet::new();
        let mut folder_owners = HashMap::new();
        let mut request_ids = HashSet::new();
        let mut environment_ids = HashSet::new();
        let mut variable_ids = HashSet::new();
        let mut snippet_ids = HashSet::new();
        let mut snippet_names = HashSet::new();

        for collection in &self.collections {
            validate_id("collection", &collection.id, &mut collection_ids)?;
            validate_name("collection", &collection.id, &collection.name)?;

            for folder in &collection.folders {
                validate_id("folder", &folder.id, &mut folder_ids)?;
                validate_name("folder", &folder.id, &folder.name)?;
                folder_owners.insert(folder.id.as_str(), collection.id.as_str());
            }
        }

        for collection in &self.collections {
            let folder_parents = collection
                .folders
                .iter()
                .map(|folder| (folder.id.as_str(), folder.parent_folder_id.as_deref()))
                .collect::<HashMap<_, _>>();

            for folder in &collection.folders {
                if let Some(parent_id) = folder.parent_folder_id.as_deref() {
                    match folder_owners.get(parent_id).copied() {
                        None => {
                            return Err(WorkspaceValidationError::DanglingFolderParent {
                                collection_id: collection.id.clone(),
                                folder_id: folder.id.clone(),
                                parent_folder_id: parent_id.to_owned(),
                            });
                        }
                        Some(parent_collection_id)
                            if parent_collection_id != collection.id.as_str() =>
                        {
                            return Err(WorkspaceValidationError::FolderParentOutsideCollection {
                                collection_id: collection.id.clone(),
                                folder_id: folder.id.clone(),
                                parent_folder_id: parent_id.to_owned(),
                                parent_collection_id: parent_collection_id.to_owned(),
                            });
                        }
                        Some(_) => {}
                    }
                }

                let mut path = HashSet::new();
                let mut current_id = Some(folder.id.as_str());
                while let Some(id) = current_id {
                    if !path.insert(id) {
                        return Err(WorkspaceValidationError::FolderCycle {
                            collection_id: collection.id.clone(),
                            folder_id: folder.id.clone(),
                        });
                    }
                    current_id = folder_parents.get(id).copied().flatten();
                }
            }

            for request in &collection.requests {
                validate_id("request", &request.id, &mut request_ids)?;
                validate_name("request", &request.id, &request.name)?;
                if let Some(folder_id) = request.folder_id.as_deref() {
                    match folder_owners.get(folder_id).copied() {
                        None => {
                            return Err(WorkspaceValidationError::DanglingRequestFolder {
                                collection_id: collection.id.clone(),
                                request_id: request.id.clone(),
                                folder_id: folder_id.to_owned(),
                            });
                        }
                        Some(folder_collection_id)
                            if folder_collection_id != collection.id.as_str() =>
                        {
                            return Err(WorkspaceValidationError::RequestFolderOutsideCollection {
                                collection_id: collection.id.clone(),
                                request_id: request.id.clone(),
                                folder_id: folder_id.to_owned(),
                                folder_collection_id: folder_collection_id.to_owned(),
                            });
                        }
                        Some(_) => {}
                    }
                }
            }
        }

        for environment in &self.environments {
            validate_id("environment", &environment.id, &mut environment_ids)?;
            validate_name("environment", &environment.id, &environment.name)?;

            let mut keys = HashSet::new();
            for variable in &environment.variables {
                validate_id("variable", &variable.id, &mut variable_ids)?;
                let key = variable.key.trim();
                if key.is_empty() {
                    return Err(WorkspaceValidationError::EmptyVariableKey {
                        environment_id: environment.id.clone(),
                        variable_id: variable.id.clone(),
                    });
                }
                if key != variable.key || key.contains("{{") || key.contains("}}") {
                    return Err(WorkspaceValidationError::InvalidVariableKey {
                        environment_id: environment.id.clone(),
                        variable_id: variable.id.clone(),
                        key: variable.key.clone(),
                    });
                }
                if !keys.insert(key) {
                    return Err(WorkspaceValidationError::DuplicateVariableKey {
                        environment_id: environment.id.clone(),
                        key: key.to_owned(),
                    });
                }
            }
        }

        if let Some(active_id) = self.active_environment_id.as_deref()
            && !environment_ids.contains(active_id)
        {
            return Err(WorkspaceValidationError::DanglingActiveEnvironment {
                id: active_id.to_owned(),
            });
        }

        for snippet in &self.snippets {
            validate_id("snippet", &snippet.id, &mut snippet_ids)?;
            snippet
                .validate()
                .map_err(|source| WorkspaceValidationError::InvalidSnippet {
                    id: snippet.id.clone(),
                    source,
                })?;
            let name_key = (snippet.category, snippet.name.to_lowercase());
            if !snippet_names.insert(name_key) {
                return Err(WorkspaceValidationError::DuplicateSnippetName {
                    category: snippet.category,
                    name: snippet.name.clone(),
                });
            }
        }

        Ok(())
    }

    pub fn snippet(&self, id: &str) -> Option<&Snippet> {
        self.snippets.iter().find(|snippet| snippet.id == id)
    }

    pub fn collection(&self, id: &str) -> Option<&Collection> {
        self.collections
            .iter()
            .find(|collection| collection.id == id)
    }

    pub fn saved_request(&self, id: &str) -> Option<(&Collection, &SavedRequest)> {
        self.collections.iter().find_map(|collection| {
            collection
                .requests
                .iter()
                .find(|request| request.id == id)
                .map(|request| (collection, request))
        })
    }

    pub fn create_collection(
        &mut self,
        name: impl Into<String>,
    ) -> Result<String, WorkspaceMutationError> {
        let collection = Collection::new(name)?;
        let id = collection.id.clone();
        self.collections.push(collection);
        Ok(id)
    }

    pub fn rename_collection(
        &mut self,
        id: &str,
        name: impl Into<String>,
    ) -> Result<(), WorkspaceMutationError> {
        let name = checked_name("collection", name.into())?;
        let collection = self
            .collections
            .iter_mut()
            .find(|collection| collection.id == id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "collection",
                id: id.to_owned(),
            })?;
        collection.name = name;
        Ok(())
    }

    /// Move a collection immediately before another collection.
    ///
    /// Passing `None` for `before_collection_id` moves the collection to the
    /// end. IDs are used instead of caller-provided vector indices so a drag
    /// operation cannot accidentally move the wrong collection after the
    /// visible tree changes.
    pub fn reorder_collection(
        &mut self,
        collection_id: &str,
        before_collection_id: Option<&str>,
    ) -> Result<(), WorkspaceMutationError> {
        let source_index = self
            .collections
            .iter()
            .position(|collection| collection.id == collection_id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "collection",
                id: collection_id.to_owned(),
            })?;

        if before_collection_id == Some(collection_id) {
            return Ok(());
        }
        if let Some(before_collection_id) = before_collection_id
            && !self
                .collections
                .iter()
                .any(|collection| collection.id == before_collection_id)
        {
            return Err(WorkspaceMutationError::NotFound {
                kind: "collection",
                id: before_collection_id.to_owned(),
            });
        }

        let collection = self.collections.remove(source_index);
        let insertion_index = before_collection_id
            .and_then(|before_collection_id| {
                self.collections
                    .iter()
                    .position(|candidate| candidate.id == before_collection_id)
            })
            .unwrap_or(self.collections.len());
        self.collections.insert(insertion_index, collection);
        Ok(())
    }

    pub fn remove_collection(&mut self, id: &str) -> Result<Collection, WorkspaceMutationError> {
        let index = self
            .collections
            .iter()
            .position(|collection| collection.id == id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "collection",
                id: id.to_owned(),
            })?;
        Ok(self.collections.remove(index))
    }

    pub fn create_collection_folder(
        &mut self,
        collection_id: &str,
        parent_folder_id: Option<&str>,
        name: impl Into<String>,
    ) -> Result<String, WorkspaceMutationError> {
        let collection = self.collection_mut(collection_id)?;
        collection.ensure_folder_exists(parent_folder_id)?;
        let folder = CollectionFolder::new(name, parent_folder_id.map(str::to_owned))?;
        let id = folder.id.clone();
        collection.folders.push(folder);
        Ok(id)
    }

    pub fn rename_collection_folder(
        &mut self,
        collection_id: &str,
        folder_id: &str,
        name: impl Into<String>,
    ) -> Result<(), WorkspaceMutationError> {
        let name = checked_name("folder", name.into())?;
        let folder = self.collection_mut(collection_id)?.folder_mut(folder_id)?;
        folder.name = name;
        Ok(())
    }

    pub fn move_collection_folder(
        &mut self,
        collection_id: &str,
        folder_id: &str,
        parent_folder_id: Option<&str>,
    ) -> Result<(), WorkspaceMutationError> {
        let collection = self.collection_mut(collection_id)?;
        collection.ensure_folder_exists(Some(folder_id))?;
        collection.ensure_folder_exists(parent_folder_id)?;

        if parent_folder_id.is_some_and(|parent_id| {
            parent_id == folder_id
                || collection
                    .folder_descendant_ids(folder_id)
                    .contains(parent_id)
        }) {
            return Err(WorkspaceMutationError::FolderCycle {
                folder_id: folder_id.to_owned(),
            });
        }

        collection.folder_mut(folder_id)?.parent_folder_id = parent_folder_id.map(str::to_owned);
        Ok(())
    }

    /// Reparent a folder and place it immediately before a sibling.
    ///
    /// `before_folder_id` must identify a folder under `parent_folder_id`.
    /// Passing `None` places the moved folder after the destination's current
    /// children. Descendant records do not need to move in the flat vector:
    /// rendering follows parent IDs, while the vector only determines sibling
    /// order.
    pub fn move_collection_folder_before(
        &mut self,
        collection_id: &str,
        folder_id: &str,
        parent_folder_id: Option<&str>,
        before_folder_id: Option<&str>,
    ) -> Result<(), WorkspaceMutationError> {
        let collection = self.collection_mut(collection_id)?;
        collection.ensure_folder_exists(Some(folder_id))?;
        collection.ensure_folder_exists(parent_folder_id)?;

        if parent_folder_id.is_some_and(|parent_id| {
            parent_id == folder_id
                || collection
                    .folder_descendant_ids(folder_id)
                    .contains(parent_id)
        }) {
            return Err(WorkspaceMutationError::FolderCycle {
                folder_id: folder_id.to_owned(),
            });
        }

        let current_parent_folder_id = collection
            .folder(folder_id)
            .expect("the source folder was validated above")
            .parent_folder_id
            .clone();
        if before_folder_id == Some(folder_id) {
            if current_parent_folder_id.as_deref() == parent_folder_id {
                return Ok(());
            }
            return Err(WorkspaceMutationError::InvalidSiblingTarget {
                kind: "folder",
                id: folder_id.to_owned(),
            });
        }
        if let Some(before_folder_id) = before_folder_id {
            let before_folder = collection.folder(before_folder_id).ok_or_else(|| {
                WorkspaceMutationError::NotFound {
                    kind: "folder",
                    id: before_folder_id.to_owned(),
                }
            })?;
            if before_folder.parent_folder_id.as_deref() != parent_folder_id {
                return Err(WorkspaceMutationError::InvalidSiblingTarget {
                    kind: "folder",
                    id: before_folder_id.to_owned(),
                });
            }
        }

        let source_index = collection
            .folders
            .iter()
            .position(|folder| folder.id == folder_id)
            .expect("the source folder was validated above");
        let mut folder = collection.folders.remove(source_index);
        folder.parent_folder_id = parent_folder_id.map(str::to_owned);

        let insertion_index = if let Some(before_folder_id) = before_folder_id {
            collection
                .folders
                .iter()
                .position(|candidate| candidate.id == before_folder_id)
                .expect("the sibling target was validated above")
        } else {
            collection
                .folders
                .iter()
                .rposition(|candidate| candidate.parent_folder_id.as_deref() == parent_folder_id)
                .map_or(source_index.min(collection.folders.len()), |index| {
                    index + 1
                })
        };
        collection.folders.insert(insertion_index, folder);
        Ok(())
    }

    /// Remove a folder, all of its descendant folders, and every request
    /// assigned anywhere in that subtree.
    pub fn remove_collection_folder(
        &mut self,
        collection_id: &str,
        folder_id: &str,
    ) -> Result<CollectionFolder, WorkspaceMutationError> {
        let collection = self.collection_mut(collection_id)?;
        let folder = collection
            .folders
            .iter()
            .find(|folder| folder.id == folder_id)
            .cloned()
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "folder",
                id: folder_id.to_owned(),
            })?;
        let removed_folder_ids = collection.folder_descendant_ids(folder_id);
        collection
            .folders
            .retain(|folder| !removed_folder_ids.contains(folder.id.as_str()));
        collection.requests.retain(|request| {
            request
                .folder_id
                .as_deref()
                .is_none_or(|id| !removed_folder_ids.contains(id))
        });
        Ok(folder)
    }

    #[allow(dead_code)]
    pub fn create_saved_request(
        &mut self,
        collection_id: &str,
        name: impl Into<String>,
        definition: RequestTemplate,
    ) -> Result<String, WorkspaceMutationError> {
        self.create_saved_request_in_folder(collection_id, None, name, definition)
    }

    pub fn create_saved_request_in_folder(
        &mut self,
        collection_id: &str,
        folder_id: Option<&str>,
        name: impl Into<String>,
        definition: RequestTemplate,
    ) -> Result<String, WorkspaceMutationError> {
        let collection = self.collection_mut(collection_id)?;
        collection.ensure_folder_exists(folder_id)?;
        let mut request = SavedRequest::new(name, definition)?;
        request.folder_id = folder_id.map(str::to_owned);
        let id = request.id.clone();
        collection.requests.push(request);
        Ok(id)
    }

    pub fn duplicate_saved_request(
        &mut self,
        collection_id: &str,
        request_id: &str,
        name: impl Into<String>,
    ) -> Result<String, WorkspaceMutationError> {
        let collection = self.collection_mut(collection_id)?;
        let source_index = collection
            .requests
            .iter()
            .position(|request| request.id == request_id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "request",
                id: request_id.to_owned(),
            })?;
        let definition = collection.requests[source_index].definition.clone();
        let folder_id = collection.requests[source_index].folder_id.clone();
        let mut duplicate = SavedRequest::new(name, definition)?;
        duplicate.folder_id = folder_id;
        let id = duplicate.id.clone();
        collection.requests.insert(source_index + 1, duplicate);
        Ok(id)
    }

    pub fn update_saved_request(
        &mut self,
        collection_id: &str,
        request_id: &str,
        definition: RequestTemplate,
    ) -> Result<(), WorkspaceMutationError> {
        let request = self.saved_request_mut(collection_id, request_id)?;
        request.definition = definition;
        request.updated_at = Utc::now();
        Ok(())
    }

    pub fn rename_saved_request(
        &mut self,
        collection_id: &str,
        request_id: &str,
        name: impl Into<String>,
    ) -> Result<(), WorkspaceMutationError> {
        let name = checked_name("request", name.into())?;
        let request = self.saved_request_mut(collection_id, request_id)?;
        request.name = name;
        request.updated_at = Utc::now();
        Ok(())
    }

    pub fn remove_saved_request(
        &mut self,
        collection_id: &str,
        request_id: &str,
    ) -> Result<SavedRequest, WorkspaceMutationError> {
        let collection = self.collection_mut(collection_id)?;
        let index = collection
            .requests
            .iter()
            .position(|request| request.id == request_id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "request",
                id: request_id.to_owned(),
            })?;
        Ok(collection.requests.remove(index))
    }

    pub fn move_saved_request(
        &mut self,
        collection_id: &str,
        request_id: &str,
        folder_id: Option<&str>,
    ) -> Result<(), WorkspaceMutationError> {
        let collection = self.collection_mut(collection_id)?;
        collection.ensure_folder_exists(folder_id)?;
        let request = collection
            .requests
            .iter_mut()
            .find(|request| request.id == request_id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "request",
                id: request_id.to_owned(),
            })?;
        request.folder_id = folder_id.map(str::to_owned);
        request.updated_at = Utc::now();
        Ok(())
    }

    /// Move a saved request to a collection/folder and place it before a
    /// sibling request.
    ///
    /// The source and destination collections may differ. Passing `None` for
    /// `before_request_id` places the request after the destination's current
    /// requests. The operation validates every destination reference before
    /// mutating either collection.
    pub fn move_saved_request_before(
        &mut self,
        source_collection_id: &str,
        request_id: &str,
        target_collection_id: &str,
        folder_id: Option<&str>,
        before_request_id: Option<&str>,
    ) -> Result<(), WorkspaceMutationError> {
        let source_collection_index = self
            .collections
            .iter()
            .position(|collection| collection.id == source_collection_id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "collection",
                id: source_collection_id.to_owned(),
            })?;
        let source_request_index = self.collections[source_collection_index]
            .requests
            .iter()
            .position(|request| request.id == request_id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "request",
                id: request_id.to_owned(),
            })?;
        let target_collection_index = self
            .collections
            .iter()
            .position(|collection| collection.id == target_collection_id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "collection",
                id: target_collection_id.to_owned(),
            })?;
        self.collections[target_collection_index].ensure_folder_exists(folder_id)?;

        let current_folder_id = self.collections[source_collection_index].requests
            [source_request_index]
            .folder_id
            .clone();
        let same_location = source_collection_index == target_collection_index
            && current_folder_id.as_deref() == folder_id;
        if before_request_id == Some(request_id) {
            if same_location {
                return Ok(());
            }
            return Err(WorkspaceMutationError::InvalidSiblingTarget {
                kind: "request",
                id: request_id.to_owned(),
            });
        }
        if let Some(before_request_id) = before_request_id {
            let before_request = self.collections[target_collection_index]
                .requests
                .iter()
                .find(|request| request.id == before_request_id)
                .ok_or_else(|| WorkspaceMutationError::NotFound {
                    kind: "request",
                    id: before_request_id.to_owned(),
                })?;
            if before_request.folder_id.as_deref() != folder_id {
                return Err(WorkspaceMutationError::InvalidSiblingTarget {
                    kind: "request",
                    id: before_request_id.to_owned(),
                });
            }
        }

        let mut request = self.collections[source_collection_index]
            .requests
            .remove(source_request_index);
        if !same_location {
            request.folder_id = folder_id.map(str::to_owned);
            request.updated_at = Utc::now();
        }

        let target_requests = &mut self.collections[target_collection_index].requests;
        let insertion_index = if let Some(before_request_id) = before_request_id {
            target_requests
                .iter()
                .position(|candidate| candidate.id == before_request_id)
                .expect("the sibling target was validated above")
        } else {
            target_requests
                .iter()
                .rposition(|candidate| candidate.folder_id.as_deref() == folder_id)
                .map_or_else(
                    || {
                        if source_collection_index == target_collection_index {
                            source_request_index.min(target_requests.len())
                        } else {
                            target_requests.len()
                        }
                    },
                    |index| index + 1,
                )
        };
        target_requests.insert(insertion_index, request);
        Ok(())
    }

    pub fn environment(&self, id: &str) -> Option<&Environment> {
        self.environments
            .iter()
            .find(|environment| environment.id == id)
    }

    pub fn active_environment(&self) -> Option<&Environment> {
        self.active_environment_id
            .as_deref()
            .and_then(|id| self.environment(id))
    }

    pub fn create_environment(
        &mut self,
        name: impl Into<String>,
    ) -> Result<String, WorkspaceMutationError> {
        let environment = Environment::new(name)?;
        let id = environment.id.clone();
        self.environments.push(environment);
        Ok(id)
    }

    pub fn rename_environment(
        &mut self,
        id: &str,
        name: impl Into<String>,
    ) -> Result<(), WorkspaceMutationError> {
        let name = checked_name("environment", name.into())?;
        let environment = self.environment_mut(id)?;
        environment.name = name;
        Ok(())
    }

    pub fn remove_environment(&mut self, id: &str) -> Result<Environment, WorkspaceMutationError> {
        let index = self
            .environments
            .iter()
            .position(|environment| environment.id == id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "environment",
                id: id.to_owned(),
            })?;
        let environment = self.environments.remove(index);
        if self.active_environment_id.as_deref() == Some(id) {
            self.active_environment_id = None;
        }
        Ok(environment)
    }

    pub fn set_active_environment(
        &mut self,
        id: Option<&str>,
    ) -> Result<(), WorkspaceMutationError> {
        if let Some(id) = id {
            if self.environment(id).is_none() {
                return Err(WorkspaceMutationError::NotFound {
                    kind: "environment",
                    id: id.to_owned(),
                });
            }
            self.active_environment_id = Some(id.to_owned());
        } else {
            self.active_environment_id = None;
        }
        Ok(())
    }

    pub fn add_environment_variable(
        &mut self,
        environment_id: &str,
        key: impl Into<String>,
        value: impl Into<String>,
        enabled: bool,
        secret: bool,
    ) -> Result<String, WorkspaceMutationError> {
        self.environment_mut(environment_id)?
            .add_variable(key, value, enabled, secret)
    }

    pub fn update_environment_variable(
        &mut self,
        environment_id: &str,
        variable_id: &str,
        key: impl Into<String>,
        value: impl Into<String>,
        enabled: bool,
        secret: bool,
    ) -> Result<(), WorkspaceMutationError> {
        self.environment_mut(environment_id)?.update_variable(
            variable_id,
            key,
            value,
            enabled,
            secret,
        )
    }

    pub fn remove_environment_variable(
        &mut self,
        environment_id: &str,
        variable_id: &str,
    ) -> Result<EnvironmentVariable, WorkspaceMutationError> {
        self.environment_mut(environment_id)?
            .remove_variable(variable_id)
    }

    fn collection_mut(&mut self, id: &str) -> Result<&mut Collection, WorkspaceMutationError> {
        self.collections
            .iter_mut()
            .find(|collection| collection.id == id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "collection",
                id: id.to_owned(),
            })
    }

    fn saved_request_mut(
        &mut self,
        collection_id: &str,
        request_id: &str,
    ) -> Result<&mut SavedRequest, WorkspaceMutationError> {
        self.collection_mut(collection_id)?
            .requests
            .iter_mut()
            .find(|request| request.id == request_id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "request",
                id: request_id.to_owned(),
            })
    }

    fn environment_mut(&mut self, id: &str) -> Result<&mut Environment, WorkspaceMutationError> {
        self.environments
            .iter_mut()
            .find(|environment| environment.id == id)
            .ok_or_else(|| WorkspaceMutationError::NotFound {
                kind: "environment",
                id: id.to_owned(),
            })
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum WorkspaceMutationError {
    #[error("{kind} name cannot be empty")]
    EmptyName { kind: &'static str },

    #[error("variable key cannot be empty")]
    EmptyVariableKey,

    #[error("invalid variable key '{key}'; keys cannot contain braces")]
    InvalidVariableKey { key: String },

    #[error("{kind} '{id}' was not found")]
    NotFound { kind: &'static str, id: String },

    #[error("folder '{folder_id}' cannot be moved beneath itself or one of its descendants")]
    FolderCycle { folder_id: String },

    #[error("{kind} '{id}' is not a sibling in the requested destination")]
    InvalidSiblingTarget { kind: &'static str, id: String },

    #[error("environment '{environment_id}' already has a variable named '{key}'")]
    DuplicateVariableKey { environment_id: String, key: String },
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum WorkspaceValidationError {
    #[error("{kind} has an empty id")]
    EmptyId { kind: &'static str },

    #[error("duplicate {kind} id '{id}'")]
    DuplicateId { kind: &'static str, id: String },

    #[error("{kind} '{id}' has an empty name")]
    EmptyName { kind: &'static str, id: String },

    #[error(
        "folder '{folder_id}' in collection '{collection_id}' references missing parent folder '{parent_folder_id}'"
    )]
    DanglingFolderParent {
        collection_id: String,
        folder_id: String,
        parent_folder_id: String,
    },

    #[error(
        "folder '{folder_id}' in collection '{collection_id}' references parent folder '{parent_folder_id}' from collection '{parent_collection_id}'"
    )]
    FolderParentOutsideCollection {
        collection_id: String,
        folder_id: String,
        parent_folder_id: String,
        parent_collection_id: String,
    },

    #[error("folder '{folder_id}' in collection '{collection_id}' forms a parent cycle")]
    FolderCycle {
        collection_id: String,
        folder_id: String,
    },

    #[error(
        "request '{request_id}' in collection '{collection_id}' references missing folder '{folder_id}'"
    )]
    DanglingRequestFolder {
        collection_id: String,
        request_id: String,
        folder_id: String,
    },

    #[error(
        "request '{request_id}' in collection '{collection_id}' references folder '{folder_id}' from collection '{folder_collection_id}'"
    )]
    RequestFolderOutsideCollection {
        collection_id: String,
        request_id: String,
        folder_id: String,
        folder_collection_id: String,
    },

    #[error("active environment '{id}' does not exist")]
    DanglingActiveEnvironment { id: String },

    #[error("variable '{variable_id}' in environment '{environment_id}' has an empty key")]
    EmptyVariableKey {
        environment_id: String,
        variable_id: String,
    },

    #[error("variable '{variable_id}' in environment '{environment_id}' has invalid key '{key}'")]
    InvalidVariableKey {
        environment_id: String,
        variable_id: String,
        key: String,
    },

    #[error("environment '{environment_id}' has duplicate variable key '{key}'")]
    DuplicateVariableKey { environment_id: String, key: String },

    #[error("snippet '{id}' is invalid")]
    InvalidSnippet {
        id: String,
        #[source]
        source: SnippetValidationError,
    },

    #[error("duplicate {category} snippet name '{name}'")]
    DuplicateSnippetName {
        category: SnippetCategory,
        name: String,
    },
}

#[cfg(test)]
#[derive(Debug, Error)]
pub enum WorkspaceStoreError {
    #[error("could not read workspace from {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not parse workspace from {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error(
        "unsupported workspace file version {found} in {path}; this build supports version {supported}"
    )]
    UnsupportedVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    #[error("workspace data is invalid")]
    Invalid(#[from] WorkspaceValidationError),

    #[error("could not create workspace directory {path}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not serialize workspace")]
    Serialize {
        #[source]
        source: serde_json::Error,
    },

    #[error("could not create temporary workspace file {path}")]
    CreateTemporary {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not write temporary workspace file {path}")]
    WriteTemporary {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not sync temporary workspace file {path}")]
    SyncTemporary {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not atomically replace {destination} with {temporary}")]
    Replace {
        destination: PathBuf,
        temporary: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub struct WorkspaceStore {
    path: PathBuf,
}

#[cfg(test)]
impl Default for WorkspaceStore {
    fn default() -> Self {
        Self::new(default_workspace_path())
    }
}

#[cfg(test)]
impl WorkspaceStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> Result<Workspace, WorkspaceStoreError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(Workspace::default());
            }
            Err(source) => {
                return Err(WorkspaceStoreError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        let file: WorkspaceFile =
            serde_json::from_slice(&bytes).map_err(|source| WorkspaceStoreError::Parse {
                path: self.path.clone(),
                source,
            })?;
        if file.version != WORKSPACE_FILE_VERSION {
            return Err(WorkspaceStoreError::UnsupportedVersion {
                path: self.path.clone(),
                found: file.version,
                supported: WORKSPACE_FILE_VERSION,
            });
        }

        let workspace = file.into_workspace();
        workspace.validate()?;
        Ok(workspace)
    }

    pub fn save(&self, workspace: &Workspace) -> Result<(), WorkspaceStoreError> {
        workspace.validate()?;

        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| WorkspaceStoreError::CreateDirectory {
                path: parent.to_owned(),
                source,
            })?;
        }

        let file = WorkspaceFile::from_workspace(workspace);
        let json = serde_json::to_vec_pretty(&file)
            .map_err(|source| WorkspaceStoreError::Serialize { source })?;
        let temporary = temporary_path_for(&self.path);

        let mut output =
            File::create(&temporary).map_err(|source| WorkspaceStoreError::CreateTemporary {
                path: temporary.clone(),
                source,
            })?;
        output
            .write_all(&json)
            .map_err(|source| WorkspaceStoreError::WriteTemporary {
                path: temporary.clone(),
                source,
            })?;
        output
            .sync_all()
            .map_err(|source| WorkspaceStoreError::SyncTemporary {
                path: temporary.clone(),
                source,
            })?;
        drop(output);

        fs::rename(&temporary, &self.path).map_err(|source| WorkspaceStoreError::Replace {
            destination: self.path.clone(),
            temporary,
            source,
        })
    }
}

#[cfg(test)]
#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceFile {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_by: Option<ResourceCreator>,
    #[serde(default)]
    collections: Vec<Collection>,
    #[serde(default)]
    environments: Vec<Environment>,
    #[serde(default)]
    active_environment_id: Option<String>,
    #[serde(default)]
    snippets: Vec<Snippet>,
}

#[cfg(test)]
impl WorkspaceFile {
    fn from_workspace(workspace: &Workspace) -> Self {
        Self {
            version: WORKSPACE_FILE_VERSION,
            created_by: workspace.created_by.clone(),
            collections: workspace.collections.clone(),
            environments: workspace.environments.clone(),
            active_environment_id: workspace.active_environment_id.clone(),
            snippets: workspace.snippets.clone(),
        }
    }

    fn into_workspace(self) -> Workspace {
        Workspace {
            created_by: self.created_by,
            collections: self.collections,
            environments: self.environments,
            active_environment_id: self.active_environment_id,
            snippets: self.snippets,
        }
    }
}

#[cfg(test)]
pub fn default_workspace_path() -> PathBuf {
    dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("API Tester")
        .join("workspace.json")
}

#[cfg(test)]
fn temporary_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace.json");
    path.with_file_name(format!(".{file_name}.tmp"))
}

fn new_id(kind: &str) -> String {
    let created_at = Utc::now();
    let sequence = NEXT_WORKSPACE_ID.fetch_add(1, Ordering::Relaxed);
    format!(
        "{kind}-{}-{}-{sequence}",
        created_at.timestamp_micros(),
        std::process::id()
    )
}

fn checked_name(kind: &'static str, name: String) -> Result<String, WorkspaceMutationError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(WorkspaceMutationError::EmptyName { kind });
    }
    Ok(name.to_owned())
}

fn checked_variable_key(key: String) -> Result<String, WorkspaceMutationError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(WorkspaceMutationError::EmptyVariableKey);
    }
    if key.contains("{{") || key.contains("}}") {
        return Err(WorkspaceMutationError::InvalidVariableKey {
            key: key.to_owned(),
        });
    }
    Ok(key.to_owned())
}

fn validate_id<'a>(
    kind: &'static str,
    id: &'a str,
    seen: &mut HashSet<&'a str>,
) -> Result<(), WorkspaceValidationError> {
    if id.trim().is_empty() {
        return Err(WorkspaceValidationError::EmptyId { kind });
    }
    if !seen.insert(id) {
        return Err(WorkspaceValidationError::DuplicateId {
            kind,
            id: id.to_owned(),
        });
    }
    Ok(())
}

fn validate_name(kind: &'static str, id: &str, name: &str) -> Result<(), WorkspaceValidationError> {
    if name.trim().is_empty() {
        return Err(WorkspaceValidationError::EmptyName {
            kind,
            id: id.to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::request::RequestDraft;
    use std::fs;

    fn template(url: &str) -> RequestTemplate {
        RequestTemplate {
            request: RequestDraft::new("GET", url),
            scripts: RequestScripts {
                pre_request: "request.headers.test = 'one';".to_owned(),
                post_response: "assert(response.status === 200);".to_owned(),
            },
        }
    }

    #[test]
    fn missing_workspace_loads_empty_without_creating_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspace.json");
        let store = WorkspaceStore::new(&path);

        assert_eq!(store.load().unwrap(), Workspace::default());
        assert!(!path.exists());
    }

    #[test]
    fn workspace_round_trips_collections_scripts_and_environment() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspace.json");
        let store = WorkspaceStore::new(&path);
        let mut workspace = Workspace::default();

        let collection_id = workspace.create_collection("My API").unwrap();
        let folder_id = workspace
            .create_collection_folder(&collection_id, None, "Users")
            .unwrap();
        let request_id = workspace
            .create_saved_request_in_folder(
                &collection_id,
                Some(&folder_id),
                "List users",
                template("{{base_url}}/users"),
            )
            .unwrap();
        let environment_id = workspace.create_environment("Development").unwrap();
        workspace
            .add_environment_variable(
                &environment_id,
                "base_url",
                "https://dev.example.com",
                true,
                false,
            )
            .unwrap();
        workspace
            .add_environment_variable(&environment_id, "token", "secret-value", true, true)
            .unwrap();
        workspace
            .set_active_environment(Some(&environment_id))
            .unwrap();

        store.save(&workspace).unwrap();
        let loaded = store.load().unwrap();

        assert_eq!(loaded, workspace);
        assert_eq!(
            loaded
                .saved_request(&request_id)
                .unwrap()
                .1
                .folder_id
                .as_deref(),
            Some(folder_id.as_str())
        );
        assert_eq!(
            loaded
                .saved_request(&request_id)
                .unwrap()
                .1
                .definition
                .scripts,
            RequestScripts {
                pre_request: "request.headers.test = 'one';".to_owned(),
                post_response: "assert(response.status === 200);".to_owned(),
            }
        );
        assert!(
            serde_json::to_string(&loaded)
                .unwrap()
                .contains("{{base_url}}")
        );
        assert!(!directory.path().join(".workspace.json.tmp").exists());
    }

    #[test]
    fn save_atomically_replaces_existing_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspace.json");
        let store = WorkspaceStore::new(&path);
        let mut first = Workspace::default();
        first.create_collection("First").unwrap();
        store.save(&first).unwrap();

        let mut second = Workspace::default();
        second.create_collection("Second").unwrap();
        store.save(&second).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.collections.len(), 1);
        assert_eq!(loaded.collections[0].name, "Second");
        assert!(!directory.path().join(".workspace.json.tmp").exists());
    }

    #[test]
    fn unsupported_version_is_rejected_without_changing_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspace.json");
        let original = br#"{"version":999,"collections":[]}"#;
        fs::write(&path, original).unwrap();
        let store = WorkspaceStore::new(&path);

        assert!(matches!(
            store.load(),
            Err(WorkspaceStoreError::UnsupportedVersion {
                found: 999,
                supported: WORKSPACE_FILE_VERSION,
                ..
            })
        ));
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn old_workspace_fields_default_when_absent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspace.json");
        fs::write(&path, br#"{"version":1}"#).unwrap();

        assert_eq!(
            WorkspaceStore::new(path).load().unwrap(),
            Workspace::default()
        );
    }

    #[test]
    fn legacy_collection_and_request_json_default_to_the_collection_root() {
        let mut workspace = Workspace::default();
        let collection_id = workspace.create_collection("Legacy").unwrap();
        workspace
            .create_saved_request(
                &collection_id,
                "Root request",
                template("https://example.com"),
            )
            .unwrap();
        let mut json = serde_json::to_value(&workspace).unwrap();
        let collection = json["collections"][0].as_object_mut().unwrap();
        collection.remove("folders");
        collection["requests"][0]
            .as_object_mut()
            .unwrap()
            .remove("folder_id");

        let loaded: Workspace = serde_json::from_value(json).unwrap();
        assert_eq!(loaded, workspace);
        assert!(loaded.collections[0].folders.is_empty());
        assert_eq!(loaded.collections[0].requests[0].folder_id, None);
    }

    #[test]
    fn collection_and_saved_request_crud_preserves_stable_ids() {
        let mut workspace = Workspace::default();
        let collection_id = workspace.create_collection("Initial").unwrap();
        let request_id = workspace
            .create_saved_request(
                &collection_id,
                "Request",
                template("https://one.example.com"),
            )
            .unwrap();
        let created_at = workspace.saved_request(&request_id).unwrap().1.created_at;

        workspace
            .rename_collection(&collection_id, "Renamed")
            .unwrap();
        workspace
            .rename_saved_request(&collection_id, &request_id, "Updated request")
            .unwrap();
        workspace
            .update_saved_request(
                &collection_id,
                &request_id,
                template("https://two.example.com"),
            )
            .unwrap();

        let (_, request) = workspace.saved_request(&request_id).unwrap();
        assert_eq!(request.id, request_id);
        assert_eq!(request.created_at, created_at);
        assert_eq!(request.name, "Updated request");
        assert_eq!(request.definition.request.url, "https://two.example.com");

        let removed = workspace
            .remove_saved_request(&collection_id, &request_id)
            .unwrap();
        assert_eq!(removed.id, request_id);
        assert!(workspace.saved_request(&request_id).is_none());
        assert_eq!(
            workspace.remove_collection(&collection_id).unwrap().name,
            "Renamed"
        );
    }

    #[test]
    fn collections_reorder_by_stable_anchor_without_partial_mutation() {
        let mut workspace = Workspace::default();
        let first = workspace.create_collection("First").unwrap();
        let second = workspace.create_collection("Second").unwrap();
        let third = workspace.create_collection("Third").unwrap();
        let ids = |workspace: &Workspace| {
            workspace
                .collections
                .iter()
                .map(|collection| collection.id.clone())
                .collect::<Vec<_>>()
        };

        workspace.reorder_collection(&third, Some(&first)).unwrap();
        assert_eq!(
            ids(&workspace),
            [third.clone(), first.clone(), second.clone()]
        );

        workspace.reorder_collection(&first, None).unwrap();
        assert_eq!(
            ids(&workspace),
            [third.clone(), second.clone(), first.clone()]
        );

        let before_missing = workspace.clone();
        assert_eq!(
            workspace.reorder_collection(&second, Some("missing")),
            Err(WorkspaceMutationError::NotFound {
                kind: "collection",
                id: "missing".to_owned(),
            })
        );
        assert_eq!(workspace, before_missing);

        workspace
            .reorder_collection(&second, Some(&second))
            .unwrap();
        assert_eq!(workspace, before_missing);
    }

    #[test]
    fn duplicate_saved_request_inserts_fresh_copy_after_source() {
        let mut workspace = Workspace::default();
        let collection_id = workspace.create_collection("Requests").unwrap();
        let first_id = workspace
            .create_saved_request(
                &collection_id,
                "Before",
                template("https://before.example.com"),
            )
            .unwrap();
        let source_id = workspace
            .create_saved_request(
                &collection_id,
                "Source",
                template("https://source.example.com"),
            )
            .unwrap();
        let trailing_id = workspace
            .create_saved_request(
                &collection_id,
                "After",
                template("https://after.example.com"),
            )
            .unwrap();

        let old_timestamp = Utc::now() - chrono::Duration::days(1);
        let source = workspace
            .collections
            .first_mut()
            .unwrap()
            .requests
            .iter_mut()
            .find(|request| request.id == source_id)
            .unwrap();
        source.created_at = old_timestamp;
        source.updated_at = old_timestamp;
        let source_definition = source.definition.clone();

        let duplicate_id = workspace
            .duplicate_saved_request(&collection_id, &source_id, "Source copy")
            .unwrap();
        let requests = &workspace.collection(&collection_id).unwrap().requests;

        assert_eq!(
            requests
                .iter()
                .map(|request| request.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                first_id.as_str(),
                source_id.as_str(),
                duplicate_id.as_str(),
                trailing_id.as_str(),
            ]
        );
        let duplicate = requests
            .iter()
            .find(|request| request.id == duplicate_id)
            .unwrap();
        assert_eq!(duplicate.name, "Source copy");
        assert_eq!(duplicate.definition, source_definition);
        assert_ne!(duplicate.id, source_id);
        assert_ne!(duplicate.created_at, old_timestamp);
        assert_ne!(duplicate.updated_at, old_timestamp);
        assert_eq!(duplicate.created_at, duplicate.updated_at);
    }

    #[test]
    fn duplicate_saved_request_reports_missing_collection_or_request_without_mutation() {
        let mut workspace = Workspace::default();
        let empty_workspace = workspace.clone();

        assert_eq!(
            workspace.duplicate_saved_request("missing-collection", "missing-request", "Copy"),
            Err(WorkspaceMutationError::NotFound {
                kind: "collection",
                id: "missing-collection".to_owned(),
            })
        );
        assert_eq!(workspace, empty_workspace);

        let collection_id = workspace.create_collection("Requests").unwrap();
        let before_missing_request = workspace.clone();
        assert_eq!(
            workspace.duplicate_saved_request(&collection_id, "missing-request", "Copy"),
            Err(WorkspaceMutationError::NotFound {
                kind: "request",
                id: "missing-request".to_owned(),
            })
        );
        assert_eq!(workspace, before_missing_request);
    }

    #[test]
    fn collection_folders_support_nesting_paths_renames_and_safe_moves() {
        let mut workspace = Workspace::default();
        let collection_id = workspace.create_collection("Requests").unwrap();
        let root_id = workspace
            .create_collection_folder(&collection_id, None, "Users")
            .unwrap();
        let child_id = workspace
            .create_collection_folder(&collection_id, Some(&root_id), "Admin")
            .unwrap();
        let sibling_id = workspace
            .create_collection_folder(&collection_id, None, "Health")
            .unwrap();

        workspace
            .rename_collection_folder(&collection_id, &child_id, "Administrators")
            .unwrap();
        workspace
            .move_collection_folder(&collection_id, &sibling_id, Some(&child_id))
            .unwrap();

        let collection = workspace.collection(&collection_id).unwrap();
        assert_eq!(
            collection.folder_path_ids(&sibling_id).unwrap(),
            [root_id.clone(), child_id.clone(), sibling_id.clone()]
        );
        assert_eq!(collection.folder(&child_id).unwrap().name, "Administrators");

        let before_cycle = workspace.clone();
        assert_eq!(
            workspace.move_collection_folder(&collection_id, &root_id, Some(&sibling_id)),
            Err(WorkspaceMutationError::FolderCycle {
                folder_id: root_id.clone(),
            })
        );
        assert_eq!(workspace, before_cycle);
        workspace.validate().unwrap();
    }

    #[test]
    fn collection_folders_reorder_and_reparent_at_sibling_anchors() {
        let mut workspace = Workspace::default();
        let collection_id = workspace.create_collection("Requests").unwrap();
        let first_root = workspace
            .create_collection_folder(&collection_id, None, "First root")
            .unwrap();
        let second_root = workspace
            .create_collection_folder(&collection_id, None, "Second root")
            .unwrap();
        let first_child = workspace
            .create_collection_folder(&collection_id, Some(&first_root), "First child")
            .unwrap();
        let second_child = workspace
            .create_collection_folder(&collection_id, Some(&first_root), "Second child")
            .unwrap();
        let moving_child = workspace
            .create_collection_folder(&collection_id, Some(&second_root), "Moving child")
            .unwrap();
        let children = |workspace: &Workspace, parent_id: &str| {
            workspace
                .collection(&collection_id)
                .unwrap()
                .folders
                .iter()
                .filter(|folder| folder.parent_folder_id.as_deref() == Some(parent_id))
                .map(|folder| folder.id.clone())
                .collect::<Vec<_>>()
        };

        workspace
            .move_collection_folder_before(
                &collection_id,
                &second_child,
                Some(&first_root),
                Some(&first_child),
            )
            .unwrap();
        assert_eq!(
            children(&workspace, &first_root),
            [second_child.clone(), first_child.clone()]
        );

        workspace
            .move_collection_folder_before(
                &collection_id,
                &moving_child,
                Some(&first_root),
                Some(&first_child),
            )
            .unwrap();
        assert_eq!(
            children(&workspace, &first_root),
            [
                second_child.clone(),
                moving_child.clone(),
                first_child.clone(),
            ]
        );
        assert!(children(&workspace, &second_root).is_empty());

        workspace
            .move_collection_folder_before(&collection_id, &second_child, Some(&first_root), None)
            .unwrap();
        assert_eq!(
            children(&workspace, &first_root),
            [
                moving_child.clone(),
                first_child.clone(),
                second_child.clone(),
            ]
        );

        let before_invalid_sibling = workspace.clone();
        assert_eq!(
            workspace.move_collection_folder_before(
                &collection_id,
                &moving_child,
                Some(&first_root),
                Some(&second_root),
            ),
            Err(WorkspaceMutationError::InvalidSiblingTarget {
                kind: "folder",
                id: second_root.clone(),
            })
        );
        assert_eq!(workspace, before_invalid_sibling);

        let before_cycle = workspace.clone();
        assert_eq!(
            workspace.move_collection_folder_before(
                &collection_id,
                &first_root,
                Some(&first_child),
                None,
            ),
            Err(WorkspaceMutationError::FolderCycle {
                folder_id: first_root,
            })
        );
        assert_eq!(workspace, before_cycle);
        workspace.validate().unwrap();
    }

    #[test]
    fn saved_requests_can_move_between_root_and_nested_folders() {
        let mut workspace = Workspace::default();
        let collection_id = workspace.create_collection("Requests").unwrap();
        let folder_id = workspace
            .create_collection_folder(&collection_id, None, "Users")
            .unwrap();
        let root_request_id = workspace
            .create_saved_request(&collection_id, "Root", template("https://example.com/root"))
            .unwrap();
        let nested_request_id = workspace
            .create_saved_request_in_folder(
                &collection_id,
                Some(&folder_id),
                "Nested",
                template("https://example.com/nested"),
            )
            .unwrap();

        assert_eq!(
            workspace
                .saved_request(&root_request_id)
                .unwrap()
                .1
                .folder_id,
            None
        );
        assert_eq!(
            workspace
                .saved_request(&nested_request_id)
                .unwrap()
                .1
                .folder_id
                .as_deref(),
            Some(folder_id.as_str())
        );

        workspace
            .move_saved_request(&collection_id, &root_request_id, Some(&folder_id))
            .unwrap();
        let duplicate_id = workspace
            .duplicate_saved_request(&collection_id, &root_request_id, "Root copy")
            .unwrap();
        assert_eq!(
            workspace
                .saved_request(&duplicate_id)
                .unwrap()
                .1
                .folder_id
                .as_deref(),
            Some(folder_id.as_str())
        );

        workspace
            .move_saved_request(&collection_id, &nested_request_id, None)
            .unwrap();
        assert_eq!(
            workspace
                .saved_request(&nested_request_id)
                .unwrap()
                .1
                .folder_id,
            None
        );
        workspace.validate().unwrap();
    }

    #[test]
    fn saved_requests_reorder_and_relocate_across_collections_atomically() {
        let mut workspace = Workspace::default();
        let source_collection_id = workspace.create_collection("Source").unwrap();
        let source_folder_id = workspace
            .create_collection_folder(&source_collection_id, None, "Source folder")
            .unwrap();
        let first_source_request = workspace
            .create_saved_request(
                &source_collection_id,
                "First source",
                template("https://example.com/source/first"),
            )
            .unwrap();
        let second_source_request = workspace
            .create_saved_request(
                &source_collection_id,
                "Second source",
                template("https://example.com/source/second"),
            )
            .unwrap();

        let target_collection_id = workspace.create_collection("Target").unwrap();
        let target_folder_id = workspace
            .create_collection_folder(&target_collection_id, None, "Target folder")
            .unwrap();
        let target_root_request = workspace
            .create_saved_request(
                &target_collection_id,
                "Target root",
                template("https://example.com/target/root"),
            )
            .unwrap();
        let first_target_request = workspace
            .create_saved_request_in_folder(
                &target_collection_id,
                Some(&target_folder_id),
                "First target",
                template("https://example.com/target/first"),
            )
            .unwrap();
        let second_target_request = workspace
            .create_saved_request_in_folder(
                &target_collection_id,
                Some(&target_folder_id),
                "Second target",
                template("https://example.com/target/second"),
            )
            .unwrap();
        let old_timestamp = Utc::now() - chrono::Duration::days(1);
        workspace
            .saved_request_mut(&source_collection_id, &first_source_request)
            .unwrap()
            .updated_at = old_timestamp;

        let second_timestamp = workspace
            .saved_request(&second_source_request)
            .unwrap()
            .1
            .updated_at;
        workspace
            .move_saved_request_before(
                &source_collection_id,
                &second_source_request,
                &source_collection_id,
                None,
                Some(&first_source_request),
            )
            .unwrap();
        assert_eq!(
            workspace
                .collection(&source_collection_id)
                .unwrap()
                .requests
                .iter()
                .map(|request| request.id.as_str())
                .collect::<Vec<_>>(),
            [
                second_source_request.as_str(),
                first_source_request.as_str()
            ]
        );
        assert_eq!(
            workspace
                .saved_request(&second_source_request)
                .unwrap()
                .1
                .updated_at,
            second_timestamp,
            "pure ordering must not change request modification time"
        );

        workspace
            .move_saved_request_before(
                &source_collection_id,
                &first_source_request,
                &target_collection_id,
                Some(&target_folder_id),
                Some(&second_target_request),
            )
            .unwrap();
        let (owner, moved) = workspace.saved_request(&first_source_request).unwrap();
        assert_eq!(owner.id, target_collection_id);
        assert_eq!(moved.folder_id.as_deref(), Some(target_folder_id.as_str()));
        assert!(moved.updated_at > old_timestamp);
        assert_eq!(
            workspace
                .collection(&target_collection_id)
                .unwrap()
                .requests
                .iter()
                .filter(|request| request.folder_id.as_deref() == Some(&target_folder_id))
                .map(|request| request.id.as_str())
                .collect::<Vec<_>>(),
            [
                first_target_request.as_str(),
                first_source_request.as_str(),
                second_target_request.as_str(),
            ]
        );
        assert_eq!(
            workspace
                .collection(&target_collection_id)
                .unwrap()
                .requests
                .iter()
                .filter(|request| request.folder_id.is_none())
                .map(|request| request.id.as_str())
                .collect::<Vec<_>>(),
            [target_root_request.as_str()]
        );

        let before_invalid_sibling = workspace.clone();
        assert_eq!(
            workspace.move_saved_request_before(
                &target_collection_id,
                &first_source_request,
                &target_collection_id,
                Some(&target_folder_id),
                Some(&target_root_request),
            ),
            Err(WorkspaceMutationError::InvalidSiblingTarget {
                kind: "request",
                id: target_root_request,
            })
        );
        assert_eq!(workspace, before_invalid_sibling);

        let before_wrong_collection_folder = workspace.clone();
        assert_eq!(
            workspace.move_saved_request_before(
                &source_collection_id,
                &second_source_request,
                &target_collection_id,
                Some(&source_folder_id),
                None,
            ),
            Err(WorkspaceMutationError::NotFound {
                kind: "folder",
                id: source_folder_id,
            })
        );
        assert_eq!(workspace, before_wrong_collection_folder);
        workspace.validate().unwrap();
    }

    #[test]
    fn removing_collection_folder_cascades_to_descendants_and_their_requests() {
        let mut workspace = Workspace::default();
        let collection_id = workspace.create_collection("Requests").unwrap();
        let removed_root_id = workspace
            .create_collection_folder(&collection_id, None, "Removed")
            .unwrap();
        let removed_child_id = workspace
            .create_collection_folder(&collection_id, Some(&removed_root_id), "Child")
            .unwrap();
        let retained_folder_id = workspace
            .create_collection_folder(&collection_id, None, "Retained")
            .unwrap();
        let root_request_id = workspace
            .create_saved_request(
                &collection_id,
                "Collection root",
                template("https://example.com/root"),
            )
            .unwrap();
        let removed_request_id = workspace
            .create_saved_request_in_folder(
                &collection_id,
                Some(&removed_child_id),
                "Removed request",
                template("https://example.com/removed"),
            )
            .unwrap();
        let retained_request_id = workspace
            .create_saved_request_in_folder(
                &collection_id,
                Some(&retained_folder_id),
                "Retained request",
                template("https://example.com/retained"),
            )
            .unwrap();

        let removed = workspace
            .remove_collection_folder(&collection_id, &removed_root_id)
            .unwrap();
        assert_eq!(removed.id, removed_root_id);
        let collection = workspace.collection(&collection_id).unwrap();
        assert!(collection.folder(&removed_root_id).is_none());
        assert!(collection.folder(&removed_child_id).is_none());
        assert!(collection.folder(&retained_folder_id).is_some());
        assert!(workspace.saved_request(&removed_request_id).is_none());
        assert!(workspace.saved_request(&root_request_id).is_some());
        assert!(workspace.saved_request(&retained_request_id).is_some());
        workspace.validate().unwrap();
    }

    #[test]
    fn validation_rejects_invalid_folder_graphs_and_request_ownership() {
        let mut workspace = Workspace {
            collections: vec![
                Collection {
                    id: "one".to_owned(),
                    name: "One".to_owned(),
                    created_by: None,
                    folders: vec![CollectionFolder {
                        id: "folder-one".to_owned(),
                        name: "One".to_owned(),
                        created_by: None,
                        parent_folder_id: Some("missing".to_owned()),
                    }],
                    requests: Vec::new(),
                },
                Collection {
                    id: "two".to_owned(),
                    name: "Two".to_owned(),
                    created_by: None,
                    folders: vec![CollectionFolder {
                        id: "folder-two".to_owned(),
                        name: "Two".to_owned(),
                        created_by: None,
                        parent_folder_id: None,
                    }],
                    requests: Vec::new(),
                },
            ],
            ..Workspace::default()
        };
        assert!(matches!(
            workspace.validate(),
            Err(WorkspaceValidationError::DanglingFolderParent {
                folder_id,
                parent_folder_id,
                ..
            }) if folder_id == "folder-one" && parent_folder_id == "missing"
        ));

        workspace.collections[0].folders[0].parent_folder_id = Some("folder-two".to_owned());
        assert!(matches!(
            workspace.validate(),
            Err(WorkspaceValidationError::FolderParentOutsideCollection {
                folder_id,
                parent_folder_id,
                ..
            }) if folder_id == "folder-one" && parent_folder_id == "folder-two"
        ));

        workspace.collections[0].folders[0].parent_folder_id = Some("folder-one".to_owned());
        assert!(matches!(
            workspace.validate(),
            Err(WorkspaceValidationError::FolderCycle { folder_id, .. })
                if folder_id == "folder-one"
        ));

        workspace.collections[0].folders[0].parent_folder_id = None;
        let mut request =
            SavedRequest::new("Wrong owner", template("https://example.com")).unwrap();
        request.folder_id = Some("folder-two".to_owned());
        workspace.collections[0].requests.push(request);
        assert!(matches!(
            workspace.validate(),
            Err(WorkspaceValidationError::RequestFolderOutsideCollection {
                folder_id,
                folder_collection_id,
                ..
            }) if folder_id == "folder-two" && folder_collection_id == "two"
        ));
    }

    #[test]
    fn environment_crud_clears_deleted_active_environment() {
        let mut workspace = Workspace::default();
        let environment_id = workspace.create_environment("Development").unwrap();
        let variable_id = workspace
            .add_environment_variable(
                &environment_id,
                "base_url",
                "https://one.example.com",
                true,
                false,
            )
            .unwrap();
        workspace
            .set_active_environment(Some(&environment_id))
            .unwrap();
        workspace
            .rename_environment(&environment_id, "Local")
            .unwrap();
        workspace
            .update_environment_variable(
                &environment_id,
                &variable_id,
                "base_url",
                "http://localhost:8080",
                true,
                false,
            )
            .unwrap();

        assert_eq!(workspace.active_environment().unwrap().name, "Local");
        assert_eq!(
            workspace
                .active_environment()
                .unwrap()
                .variable(&variable_id)
                .unwrap()
                .value,
            "http://localhost:8080"
        );

        let removed = workspace.remove_environment(&environment_id).unwrap();
        assert_eq!(removed.id, environment_id);
        assert_eq!(workspace.active_environment_id, None);
    }

    #[test]
    fn rejects_duplicate_variable_keys_without_partial_mutation() {
        let mut workspace = Workspace::default();
        let environment_id = workspace.create_environment("Test").unwrap();
        workspace
            .add_environment_variable(&environment_id, "token", "one", true, true)
            .unwrap();

        assert_eq!(
            workspace.add_environment_variable(&environment_id, " token ", "two", true, true),
            Err(WorkspaceMutationError::DuplicateVariableKey {
                environment_id: environment_id.clone(),
                key: "token".to_owned(),
            })
        );
        assert_eq!(
            workspace
                .environment(&environment_id)
                .unwrap()
                .variables
                .len(),
            1
        );
    }

    #[test]
    fn validates_duplicate_ids_keys_and_dangling_active_environment() {
        let mut workspace = Workspace {
            collections: vec![
                Collection {
                    id: "same".to_owned(),
                    name: "One".to_owned(),
                    created_by: None,
                    folders: Vec::new(),
                    requests: Vec::new(),
                },
                Collection {
                    id: "same".to_owned(),
                    name: "Two".to_owned(),
                    created_by: None,
                    folders: Vec::new(),
                    requests: Vec::new(),
                },
            ],
            ..Workspace::default()
        };
        assert_eq!(
            workspace.validate(),
            Err(WorkspaceValidationError::DuplicateId {
                kind: "collection",
                id: "same".to_owned(),
            })
        );

        workspace.collections.clear();
        workspace.active_environment_id = Some("missing".to_owned());
        assert_eq!(
            workspace.validate(),
            Err(WorkspaceValidationError::DanglingActiveEnvironment {
                id: "missing".to_owned(),
            })
        );

        workspace.active_environment_id = None;
        workspace.environments.push(Environment {
            id: "environment".to_owned(),
            name: "Test".to_owned(),
            created_by: None,
            variables: vec![
                EnvironmentVariable {
                    id: "one".to_owned(),
                    key: "token".to_owned(),
                    value: "one".to_owned(),
                    enabled: true,
                    secret: false,
                    created_by: None,
                },
                EnvironmentVariable {
                    id: "two".to_owned(),
                    key: "token".to_owned(),
                    value: "two".to_owned(),
                    enabled: false,
                    secret: false,
                    created_by: None,
                },
            ],
        });
        assert_eq!(
            workspace.validate(),
            Err(WorkspaceValidationError::DuplicateVariableKey {
                environment_id: "environment".to_owned(),
                key: "token".to_owned(),
            })
        );
    }

    #[test]
    fn persisted_workspace_has_explicit_version_and_not_history_shape() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspace.json");
        WorkspaceStore::new(&path)
            .save(&Workspace::default())
            .unwrap();

        let json: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(json["version"], WORKSPACE_FILE_VERSION);
        assert!(json.get("entries").is_none());
        assert_eq!(json["collections"], serde_json::json!([]));
        assert_eq!(json["environments"], serde_json::json!([]));
    }
}
