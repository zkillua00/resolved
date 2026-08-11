package workspaces

import (
	"sort"

	"resolved-server/internal/resourceevents"
)

func (s *Service) publishChange(change resourceevents.Change) {
	resourceevents.Emit(s.events, change)
}

func ownerScopedAudience(userIDs ...[]string) resourceevents.Audience {
	return resourceevents.Audience{
		Owners:  true,
		UserIDs: unionUserIDs(userIDs...),
	}
}

func workspaceAudience(workspace Workspace) []string {
	result := make(map[string]struct{})
	addUserIDs(result, workspace.UserIDs)
	collectCollectionUserIDs(workspace.Collections, result)
	return userIDSet(result)
}

func workspaceDirectAudience(workspace Workspace) []string {
	return unionUserIDs(workspace.UserIDs)
}

func collectionAudience(workspace Workspace, collectionID string, includeDescendants bool) []string {
	result := make(map[string]struct{})
	addUserIDs(result, workspace.UserIDs)
	findCollectionAudience(workspace.Collections, collectionID, nil, includeDescendants, result)
	return userIDSet(result)
}

func findCollectionAudience(
	collections []Collection,
	collectionID string,
	inherited []string,
	includeDescendants bool,
	result map[string]struct{},
) bool {
	for _, collection := range collections {
		effective := unionUserIDs(inherited, collection.UserIDs)
		if collection.ID == collectionID {
			addUserIDs(result, effective)
			if includeDescendants {
				collectCollectionUserIDs(collection.SubCollections, result)
			}
			return true
		}
		if findCollectionAudience(
			collection.SubCollections,
			collectionID,
			effective,
			includeDescendants,
			result,
		) {
			return true
		}
	}
	return false
}

func collectCollectionUserIDs(collections []Collection, result map[string]struct{}) {
	for _, collection := range collections {
		addUserIDs(result, collection.UserIDs)
		collectCollectionUserIDs(collection.SubCollections, result)
	}
}

func unionUserIDs(groups ...[]string) []string {
	result := make(map[string]struct{})
	for _, group := range groups {
		addUserIDs(result, group)
	}
	return userIDSet(result)
}

func addUserIDs(result map[string]struct{}, userIDs []string) {
	for _, userID := range userIDs {
		if userID != "" {
			result[userID] = struct{}{}
		}
	}
}

func userIDSet(values map[string]struct{}) []string {
	result := make([]string, 0, len(values))
	for value := range values {
		result = append(result, value)
	}
	sort.Strings(result)
	return result
}
