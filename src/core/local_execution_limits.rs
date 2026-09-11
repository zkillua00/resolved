//! Sparse local policies. Collection/folder identities are namespaced by workspace.
use super::{
    Workspace,
    execution_limits::{Bound, DEFAULT_LIMITS, ExecutionLimits},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

pub type Overrides = BTreeMap<String, Bound>;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct LocalExecutionLimitSettings {
    pub application: Overrides,
    pub workspaces: BTreeMap<String, Overrides>,
    /// Both root collections and folders use their actual workspace resource IDs.
    pub collections: BTreeMap<String, BTreeMap<String, Overrides>>,
}

pub fn validate_overrides(overrides: &Overrides) -> Result<(), String> {
    for (key, bound) in overrides {
        if !DEFAULT_LIMITS.iter().any(|(name, _)| name == key) {
            return Err(format!("Unknown execution limit: {key}"));
        }
        if bound.unlimited && bound.value != 0 {
            return Err(format!("{key}: Unlimited must have value zero"));
        }
    }
    ExecutionLimits(overrides.clone()).validate()
}

impl LocalExecutionLimitSettings {
    pub fn resolve(
        &self,
        workspace_id: &str,
        workspace: &Workspace,
        collection_id: Option<&str>,
    ) -> Result<ExecutionLimits, String> {
        self.resolve_with_timeout(
            workspace_id,
            workspace,
            collection_id,
            Duration::from_millis(30_000),
        )
    }

    pub fn resolve_with_timeout(
        &self,
        workspace_id: &str,
        workspace: &Workspace,
        collection_id: Option<&str>,
        fallback_timeout: Duration,
    ) -> Result<ExecutionLimits, String> {
        self.resolve_for_request(
            workspace_id,
            workspace,
            collection_id,
            None,
            fallback_timeout,
        )
    }

    pub fn resolve_for_request(
        &self,
        workspace_id: &str,
        workspace: &Workspace,
        collection_id: Option<&str>,
        folder_id: Option<&str>,
        fallback_timeout: Duration,
    ) -> Result<ExecutionLimits, String> {
        Ok(self
            .resolve_with_sources(
                workspace_id,
                workspace,
                collection_id,
                folder_id,
                fallback_timeout,
            )?
            .0)
    }

    pub fn resolve_with_sources(
        &self,
        workspace_id: &str,
        workspace: &Workspace,
        collection_id: Option<&str>,
        folder_id: Option<&str>,
        fallback_timeout: Duration,
    ) -> Result<(ExecutionLimits, BTreeMap<String, String>), String> {
        let mut limits = ExecutionLimits::default();
        let mut sources = BTreeMap::new();
        // Preserve old settings dynamically, without converting them into a new
        // explicit override that would mask later edits to Script settings.
        if !self.application.contains_key("script.timeout_ms") {
            let value = i64::try_from(fallback_timeout.as_millis())
                .map_err(|_| "Legacy script timeout exceeds supported milliseconds")?;
            limits
                .0
                .insert("script.timeout_ms".into(), Bound::limited(value));
            sources.insert(
                "script.timeout_ms".into(),
                "Application script timeout".into(),
            );
        }
        let mut overlay = |values: &Overrides, source: String| -> Result<(), String> {
            validate_overrides(values)?;
            for (key, value) in values {
                limits.0.insert(key.clone(), *value);
                sources.insert(key.clone(), source.clone());
            }
            Ok(())
        };
        overlay(&self.application, "Application".into())?;
        if let Some(values) = self.workspaces.get(workspace_id) {
            overlay(values, "Workspace".into())?;
        }
        let path = local_scope_path(workspace, collection_id, folder_id)?;
        if let Some(scopes) = self.collections.get(workspace_id) {
            for (id, name) in path {
                if let Some(values) = scopes.get(&id) {
                    overlay(values, name)?;
                }
            }
        }
        limits.validate()?;
        Ok((limits, sources))
    }
}

/// Validate actual identities before looking up policy. Never accept a folder
/// from another collection or silently truncate a missing/cyclic ancestor.
pub fn local_scope_path(
    workspace: &Workspace,
    collection_id: Option<&str>,
    folder_id: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    let Some(collection_id) = collection_id else {
        return if folder_id.is_some() {
            Err("Folder requires a collection".into())
        } else {
            Ok(Vec::new())
        };
    };
    let mut ids = BTreeSet::new();
    for collection in &workspace.collections {
        if !ids.insert(collection.id.as_str()) {
            return Err("Ambiguous collection identity".into());
        }
        for folder in &collection.folders {
            if !ids.insert(folder.id.as_str()) {
                return Err("Ambiguous folder identity".into());
            }
        }
    }
    let collection = workspace
        .collections
        .iter()
        .find(|c| c.id == collection_id)
        .ok_or_else(|| format!("Unknown local collection: {collection_id}"))?;
    let mut path = vec![(collection.id.clone(), collection.name.clone())];
    if let Some(folder_id) = folder_id {
        for id in collection
            .folder_path_ids(folder_id)
            .map_err(|e| e.to_string())?
        {
            let folder = collection.folder(&id).ok_or("Missing local folder")?;
            path.push((id, folder.name.clone()));
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{AppSettings, Collection, CollectionFolder};

    #[test]
    fn legacy_settings_and_roundtrip() {
        let old: AppSettings = serde_json::from_str(r#"{"script":{"timeout_ms":98765}}"#).unwrap();
        let effective = old
            .execution_limits
            .resolve_with_timeout("w", &Workspace::default(), None, old.script.timeout())
            .unwrap();
        assert_eq!(effective.get("script.timeout_ms"), Bound::limited(98765));
        assert_eq!(
            old,
            serde_json::from_str(&serde_json::to_string(&old).unwrap()).unwrap()
        );
    }

    #[test]
    fn explicit_timeout_zero_and_unlimited_replace_legacy_timeout() {
        let mut settings = LocalExecutionLimitSettings::default();
        for bound in [Bound::limited(0), Bound::unlimited()] {
            settings
                .application
                .insert("script.timeout_ms".into(), bound);
            assert_eq!(
                settings
                    .resolve_with_timeout(
                        "w",
                        &Workspace::default(),
                        None,
                        Duration::from_secs(123)
                    )
                    .unwrap()
                    .get("script.timeout_ms"),
                bound
            );
        }
    }

    #[test]
    fn cyclic_and_cross_collection_ancestry_is_rejected() {
        let mut root = Collection::new("Root").unwrap();
        let mut folder = CollectionFolder::new("Folder", None).unwrap();
        folder.parent_folder_id = Some(folder.id.clone());
        root.folders.push(folder.clone());
        let other = Collection::new("Other").unwrap();
        let workspace = Workspace {
            collections: vec![root.clone(), other.clone()],
            ..Default::default()
        };
        assert!(local_scope_path(&workspace, Some(&root.id), Some(&folder.id)).is_err());
        assert!(local_scope_path(&workspace, Some(&other.id), Some(&folder.id)).is_err());
    }

    #[test]
    fn nearest_wins_and_workspace_isolation() {
        let mut root = Collection::new("Root").unwrap();
        let parent = CollectionFolder::new("Parent", None).unwrap();
        let child = CollectionFolder::new("Child", Some(parent.id.clone())).unwrap();
        root.folders = vec![parent.clone(), child.clone()];
        let workspace = Workspace {
            collections: vec![root.clone()],
            ..Default::default()
        };
        let mut settings = LocalExecutionLimitSettings::default();
        let key = "http.response_bytes";
        settings.application.insert(key.into(), Bound::limited(10));
        settings.workspaces.insert(
            "w".into(),
            BTreeMap::from([(key.into(), Bound::limited(20))]),
        );
        settings.collections.insert(
            "w".into(),
            BTreeMap::from([
                (
                    root.id.clone(),
                    BTreeMap::from([(key.into(), Bound::limited(5))]),
                ),
                (
                    parent.id.clone(),
                    BTreeMap::from([(key.into(), Bound::unlimited())]),
                ),
                (
                    child.id.clone(),
                    BTreeMap::from([(key.into(), Bound::limited(100))]),
                ),
            ]),
        );
        let resolve = |w, f| {
            settings
                .resolve_for_request(w, &workspace, Some(&root.id), f, Duration::from_secs(30))
                .unwrap()
                .get(key)
        };
        assert_eq!(resolve("w", None), Bound::limited(5));
        assert_eq!(resolve("w", Some(&parent.id)), Bound::unlimited());
        assert_eq!(resolve("w", Some(&child.id)), Bound::limited(100));
        assert_eq!(resolve("other", Some(&child.id)), Bound::limited(10));
        assert!(
            settings
                .resolve_for_request(
                    "w",
                    &workspace,
                    Some(&root.id),
                    Some("missing"),
                    Duration::ZERO
                )
                .is_err()
        );
        assert_eq!(
            settings,
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap()
        );
    }

    #[test]
    fn malformed_updates_are_rejected() {
        for (key, value) in [
            ("unknown", Bound::limited(1)),
            ("http.redirects", Bound::limited(-1)),
            (
                "http.redirects",
                Bound {
                    unlimited: true,
                    value: 1,
                },
            ),
        ] {
            assert!(validate_overrides(&BTreeMap::from([(key.into(), value)])).is_err());
        }
        assert!(
            validate_overrides(&BTreeMap::from([(
                "http.redirects".into(),
                Bound::limited(0)
            )]))
            .is_ok()
        );
    }
}
