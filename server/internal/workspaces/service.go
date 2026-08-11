package workspaces

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"strings"
	"unicode/utf8"

	"resolved-server/internal/problem"

	"github.com/google/uuid"
)

type Service struct {
	repository *Repository
}

type Actor struct {
	UserID string
	Owner  bool
}

type CreateWorkspaceInput struct {
	Name string
}

type UpdateWorkspaceInput struct {
	Name string
}

type CreateCollectionInput struct {
	Name               string
	ParentCollectionID *string
}

type UpdateCollectionInput struct {
	Name string
}

type CreateSavedRequestInput struct {
	Name       string
	Definition json.RawMessage
}

type UpdateSavedRequestInput struct {
	Name       string
	Definition json.RawMessage
}

func NewService(repository *Repository) *Service {
	return &Service{repository: repository}
}

func (s *Service) List(ctx context.Context, actor Actor) ([]Workspace, error) {
	workspaces, err := s.repository.ListWorkspaces(ctx)
	if err != nil {
		return nil, problem.Wrap(err, "list workspaces")
	}
	visible := make([]Workspace, 0, len(workspaces))
	for _, workspace := range workspaces {
		if scoped, ok := scopeWorkspace(workspace, actor); ok {
			visible = append(visible, scoped)
		}
	}
	return visible, nil
}

func (s *Service) Get(ctx context.Context, actor Actor, id string) (Workspace, error) {
	if err := validateID("workspace_id", id); err != nil {
		return Workspace{}, err
	}
	workspace, err := s.repository.GetWorkspace(ctx, id)
	if err != nil {
		return Workspace{}, mapRepositoryError(err)
	}
	scoped, ok := scopeWorkspace(workspace, actor)
	if !ok {
		return Workspace{}, workspaceAccessDenied()
	}
	return scoped, nil
}

func (s *Service) Create(ctx context.Context, actor Actor, input CreateWorkspaceInput) (Workspace, error) {
	name, err := normalizeName(input.Name)
	if err != nil {
		return Workspace{}, err
	}
	creatorID := actor.UserID
	workspace := Workspace{ID: uuid.NewString(), Name: name, CreatedByUserID: &creatorID}
	created, err := s.repository.CreateWorkspace(ctx, workspace, []string{actor.UserID})
	if err != nil {
		return Workspace{}, mapRepositoryError(err)
	}
	return created, nil
}

func (s *Service) Update(
	ctx context.Context,
	actor Actor,
	id string,
	input UpdateWorkspaceInput,
) (Workspace, error) {
	if err := validateID("workspace_id", id); err != nil {
		return Workspace{}, err
	}
	name, err := normalizeName(input.Name)
	if err != nil {
		return Workspace{}, err
	}
	workspace, err := s.repository.GetWorkspace(ctx, id)
	if err != nil {
		return Workspace{}, mapRepositoryError(err)
	}
	if !hasWorkspaceGrant(workspace, actor) {
		return Workspace{}, workspaceAccessDenied()
	}
	updated, err := s.repository.UpdateWorkspaceName(ctx, id, name)
	if err != nil {
		return Workspace{}, mapRepositoryError(err)
	}
	return updated, nil
}

func (s *Service) ReplaceWorkspaceUsers(
	ctx context.Context,
	actor Actor,
	id string,
	userIDs []string,
) (Workspace, error) {
	if err := validateID("workspace_id", id); err != nil {
		return Workspace{}, err
	}
	if err := validateIDs("user_ids", userIDs); err != nil {
		return Workspace{}, err
	}
	workspace, err := s.repository.GetWorkspace(ctx, id)
	if err != nil {
		return Workspace{}, mapRepositoryError(err)
	}
	if !hasWorkspaceGrant(workspace, actor) {
		return Workspace{}, workspaceAccessDenied()
	}
	updated, err := s.repository.ReplaceWorkspaceUsers(ctx, id, userIDs)
	if err != nil {
		return Workspace{}, mapRepositoryError(err)
	}
	return updated, nil
}

func (s *Service) Delete(ctx context.Context, actor Actor, id string) error {
	if err := validateID("workspace_id", id); err != nil {
		return err
	}
	workspace, err := s.repository.GetWorkspace(ctx, id)
	if err != nil {
		return mapRepositoryError(err)
	}
	if !hasWorkspaceGrant(workspace, actor) {
		return workspaceAccessDenied()
	}
	if err := s.repository.DeleteWorkspace(ctx, id); err != nil {
		return mapRepositoryError(err)
	}
	return nil
}

func (s *Service) GetCollection(
	ctx context.Context,
	actor Actor,
	workspaceID, collectionID string,
) (Collection, error) {
	if err := validateWorkspaceAndCollectionIDs(workspaceID, collectionID); err != nil {
		return Collection{}, err
	}
	workspace, err := s.repository.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return Collection{}, mapRepositoryError(err)
	}
	collection, exists := findCollection(workspace.Collections, collectionID)
	if !exists {
		return Collection{}, mapRepositoryError(ErrCollectionNotFound)
	}
	if !hasCollectionGrant(workspace, collectionID, actor) {
		return Collection{}, collectionAccessDenied()
	}
	return collection, nil
}

func (s *Service) CreateCollection(
	ctx context.Context,
	actor Actor,
	workspaceID string,
	input CreateCollectionInput,
) (Collection, error) {
	if err := validateID("workspace_id", workspaceID); err != nil {
		return Collection{}, err
	}
	name, err := normalizeName(input.Name)
	if err != nil {
		return Collection{}, err
	}
	if input.ParentCollectionID != nil {
		if err := validateID("parent_collection_id", *input.ParentCollectionID); err != nil {
			return Collection{}, err
		}
	}
	workspace, err := s.repository.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return Collection{}, mapRepositoryError(err)
	}
	if input.ParentCollectionID == nil {
		if !hasWorkspaceGrant(workspace, actor) {
			return Collection{}, workspaceAccessDenied()
		}
	} else {
		if _, exists := findCollection(workspace.Collections, *input.ParentCollectionID); !exists {
			return Collection{}, mapRepositoryError(ErrParentCollectionMissing)
		}
		if !hasCollectionGrant(workspace, *input.ParentCollectionID, actor) {
			return Collection{}, collectionAccessDenied()
		}
	}
	collection := Collection{
		ID:                 uuid.NewString(),
		WorkspaceID:        workspaceID,
		ParentCollectionID: input.ParentCollectionID,
		Name:               name,
		CreatedByUserID:    &actor.UserID,
	}
	created, err := s.repository.CreateCollection(ctx, collection)
	if err != nil {
		return Collection{}, mapRepositoryError(err)
	}
	return created, nil
}

func (s *Service) UpdateCollection(
	ctx context.Context,
	actor Actor,
	workspaceID, collectionID string,
	input UpdateCollectionInput,
) (Collection, error) {
	if err := validateWorkspaceAndCollectionIDs(workspaceID, collectionID); err != nil {
		return Collection{}, err
	}
	name, err := normalizeName(input.Name)
	if err != nil {
		return Collection{}, err
	}
	workspace, err := s.repository.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return Collection{}, mapRepositoryError(err)
	}
	if _, exists := findCollection(workspace.Collections, collectionID); !exists {
		return Collection{}, mapRepositoryError(ErrCollectionNotFound)
	}
	if !hasCollectionGrant(workspace, collectionID, actor) {
		return Collection{}, collectionAccessDenied()
	}
	updated, err := s.repository.UpdateCollectionName(ctx, workspaceID, collectionID, name)
	if err != nil {
		return Collection{}, mapRepositoryError(err)
	}
	return updated, nil
}

func (s *Service) MoveCollection(
	ctx context.Context,
	actor Actor,
	workspaceID, collectionID string,
	parentCollectionID *string,
) (Collection, error) {
	if err := validateWorkspaceAndCollectionIDs(workspaceID, collectionID); err != nil {
		return Collection{}, err
	}
	if parentCollectionID != nil {
		if err := validateID("parent_collection_id", *parentCollectionID); err != nil {
			return Collection{}, err
		}
	}
	workspace, err := s.repository.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return Collection{}, mapRepositoryError(err)
	}
	if _, exists := findCollection(workspace.Collections, collectionID); !exists {
		return Collection{}, mapRepositoryError(ErrCollectionNotFound)
	}
	if !hasCollectionGrant(workspace, collectionID, actor) {
		return Collection{}, collectionAccessDenied()
	}
	if parentCollectionID == nil {
		if !hasWorkspaceGrant(workspace, actor) {
			return Collection{}, workspaceAccessDenied()
		}
	} else {
		if _, exists := findCollection(workspace.Collections, *parentCollectionID); !exists {
			return Collection{}, mapRepositoryError(ErrParentCollectionMissing)
		}
		if !hasCollectionGrant(workspace, *parentCollectionID, actor) {
			return Collection{}, collectionAccessDenied()
		}
	}
	moved, err := s.repository.MoveCollection(ctx, workspaceID, collectionID, parentCollectionID)
	if err != nil {
		return Collection{}, mapRepositoryError(err)
	}
	return moved, nil
}

func (s *Service) ReplaceCollectionUsers(
	ctx context.Context,
	actor Actor,
	workspaceID, collectionID string,
	userIDs []string,
) (Collection, error) {
	if err := validateWorkspaceAndCollectionIDs(workspaceID, collectionID); err != nil {
		return Collection{}, err
	}
	if err := validateIDs("user_ids", userIDs); err != nil {
		return Collection{}, err
	}
	workspace, err := s.repository.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return Collection{}, mapRepositoryError(err)
	}
	if _, exists := findCollection(workspace.Collections, collectionID); !exists {
		return Collection{}, mapRepositoryError(ErrCollectionNotFound)
	}
	if !hasCollectionGrant(workspace, collectionID, actor) {
		return Collection{}, collectionAccessDenied()
	}
	updated, err := s.repository.ReplaceCollectionUsers(ctx, workspaceID, collectionID, userIDs)
	if err != nil {
		return Collection{}, mapRepositoryError(err)
	}
	return updated, nil
}

func (s *Service) DeleteCollection(
	ctx context.Context,
	actor Actor,
	workspaceID, collectionID string,
) error {
	if err := validateWorkspaceAndCollectionIDs(workspaceID, collectionID); err != nil {
		return err
	}
	workspace, err := s.repository.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return mapRepositoryError(err)
	}
	if _, exists := findCollection(workspace.Collections, collectionID); !exists {
		return mapRepositoryError(ErrCollectionNotFound)
	}
	if !hasCollectionGrant(workspace, collectionID, actor) {
		return collectionAccessDenied()
	}
	if err := s.repository.DeleteCollection(ctx, workspaceID, collectionID); err != nil {
		return mapRepositoryError(err)
	}
	return nil
}

func (s *Service) GetSavedRequest(
	ctx context.Context,
	actor Actor,
	workspaceID, collectionID, requestID string,
) (SavedRequest, error) {
	if err := validateWorkspaceCollectionRequestIDs(workspaceID, collectionID, requestID); err != nil {
		return SavedRequest{}, err
	}
	workspace, err := s.repository.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return SavedRequest{}, mapRepositoryError(err)
	}
	if _, exists := findCollection(workspace.Collections, collectionID); !exists {
		return SavedRequest{}, mapRepositoryError(ErrCollectionNotFound)
	}
	if !hasCollectionGrant(workspace, collectionID, actor) {
		return SavedRequest{}, collectionAccessDenied()
	}
	request, err := s.repository.GetSavedRequest(ctx, workspaceID, collectionID, requestID)
	if err != nil {
		return SavedRequest{}, mapRepositoryError(err)
	}
	return request, nil
}

func (s *Service) CreateSavedRequest(
	ctx context.Context,
	actor Actor,
	workspaceID, collectionID string,
	input CreateSavedRequestInput,
) (SavedRequest, error) {
	if err := validateWorkspaceAndCollectionIDs(workspaceID, collectionID); err != nil {
		return SavedRequest{}, err
	}
	name, err := normalizeName(input.Name)
	if err != nil {
		return SavedRequest{}, err
	}
	definition, err := normalizeRequestDefinition(input.Definition)
	if err != nil {
		return SavedRequest{}, err
	}
	workspace, err := s.repository.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return SavedRequest{}, mapRepositoryError(err)
	}
	if _, exists := findCollection(workspace.Collections, collectionID); !exists {
		return SavedRequest{}, mapRepositoryError(ErrCollectionNotFound)
	}
	if !hasCollectionGrant(workspace, collectionID, actor) {
		return SavedRequest{}, collectionAccessDenied()
	}
	created, err := s.repository.CreateSavedRequest(ctx, workspaceID, SavedRequest{
		ID:              uuid.NewString(),
		CollectionID:    collectionID,
		Name:            name,
		Definition:      definition,
		CreatedByUserID: &actor.UserID,
	})
	if err != nil {
		return SavedRequest{}, mapRepositoryError(err)
	}
	return created, nil
}

func (s *Service) UpdateSavedRequest(
	ctx context.Context,
	actor Actor,
	workspaceID, collectionID, requestID string,
	input UpdateSavedRequestInput,
) (SavedRequest, error) {
	if err := validateWorkspaceCollectionRequestIDs(workspaceID, collectionID, requestID); err != nil {
		return SavedRequest{}, err
	}
	name, err := normalizeName(input.Name)
	if err != nil {
		return SavedRequest{}, err
	}
	definition, err := normalizeRequestDefinition(input.Definition)
	if err != nil {
		return SavedRequest{}, err
	}
	workspace, err := s.repository.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return SavedRequest{}, mapRepositoryError(err)
	}
	if _, exists := findCollection(workspace.Collections, collectionID); !exists {
		return SavedRequest{}, mapRepositoryError(ErrCollectionNotFound)
	}
	if !hasCollectionGrant(workspace, collectionID, actor) {
		return SavedRequest{}, collectionAccessDenied()
	}
	updated, err := s.repository.UpdateSavedRequest(
		ctx, workspaceID, collectionID, requestID, name, definition,
	)
	if err != nil {
		return SavedRequest{}, mapRepositoryError(err)
	}
	return updated, nil
}

func (s *Service) DeleteSavedRequest(
	ctx context.Context,
	actor Actor,
	workspaceID, collectionID, requestID string,
) error {
	if err := validateWorkspaceCollectionRequestIDs(workspaceID, collectionID, requestID); err != nil {
		return err
	}
	workspace, err := s.repository.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return mapRepositoryError(err)
	}
	if _, exists := findCollection(workspace.Collections, collectionID); !exists {
		return mapRepositoryError(ErrCollectionNotFound)
	}
	if !hasCollectionGrant(workspace, collectionID, actor) {
		return collectionAccessDenied()
	}
	if err := s.repository.DeleteSavedRequest(ctx, workspaceID, collectionID, requestID); err != nil {
		return mapRepositoryError(err)
	}
	return nil
}

func scopeWorkspace(workspace Workspace, actor Actor) (Workspace, bool) {
	if hasWorkspaceGrant(workspace, actor) {
		return workspace, true
	}
	effective := effectiveCollectionIDs(workspace.Collections, actor.UserID, false, nil)
	if len(effective) == 0 {
		return Workspace{}, false
	}
	parents := make(map[string]*string)
	collectParents(workspace.Collections, parents)
	visible := make(map[string]struct{}, len(effective))
	for id := range effective {
		visible[id] = struct{}{}
		current := parents[id]
		seen := map[string]struct{}{id: {}}
		for current != nil {
			if _, exists := seen[*current]; exists {
				break
			}
			seen[*current] = struct{}{}
			visible[*current] = struct{}{}
			current = parents[*current]
		}
	}

	scoped := workspace
	scoped.UserIDs = []string{}
	scoped.CreatedByUser = nil
	scoped.Collections = filterCollections(workspace.Collections, visible, effective)
	return scoped, true
}

func hasWorkspaceGrant(workspace Workspace, actor Actor) bool {
	return actor.Owner || containsString(workspace.UserIDs, actor.UserID)
}

func hasCollectionGrant(workspace Workspace, collectionID string, actor Actor) bool {
	if hasWorkspaceGrant(workspace, actor) {
		return true
	}
	_, exists := effectiveCollectionIDs(workspace.Collections, actor.UserID, false, nil)[collectionID]
	return exists
}

func effectiveCollectionIDs(
	collections []Collection,
	userID string,
	inherited bool,
	result map[string]struct{},
) map[string]struct{} {
	if result == nil {
		result = make(map[string]struct{})
	}
	for _, collection := range collections {
		effective := inherited || containsString(collection.UserIDs, userID)
		if effective {
			result[collection.ID] = struct{}{}
		}
		effectiveCollectionIDs(collection.SubCollections, userID, effective, result)
	}
	return result
}

func collectParents(collections []Collection, parents map[string]*string) {
	for _, collection := range collections {
		parents[collection.ID] = collection.ParentCollectionID
		collectParents(collection.SubCollections, parents)
	}
}

func filterCollections(
	collections []Collection,
	visible map[string]struct{},
	effective map[string]struct{},
) []Collection {
	filtered := make([]Collection, 0, len(collections))
	for _, collection := range collections {
		if _, ok := visible[collection.ID]; !ok {
			continue
		}
		copy := collection
		if _, ok := effective[collection.ID]; !ok {
			copy.UserIDs = []string{}
			copy.Requests = []SavedRequest{}
			copy.CreatedByUser = nil
		}
		copy.SubCollections = filterCollections(collection.SubCollections, visible, effective)
		filtered = append(filtered, copy)
	}
	return filtered
}

func containsString(values []string, expected string) bool {
	for _, value := range values {
		if value == expected {
			return true
		}
	}
	return false
}

func normalizeName(value string) (string, error) {
	name := strings.TrimSpace(value)
	length := utf8.RuneCountInString(name)
	if length == 0 || length > 120 {
		return "", problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"name": "must contain between 1 and 120 characters"},
		)
	}
	return name, nil
}

func validateWorkspaceAndCollectionIDs(workspaceID, collectionID string) error {
	if err := validateID("workspace_id", workspaceID); err != nil {
		return err
	}
	return validateID("collection_id", collectionID)
}

func validateWorkspaceCollectionRequestIDs(workspaceID, collectionID, requestID string) error {
	if err := validateWorkspaceAndCollectionIDs(workspaceID, collectionID); err != nil {
		return err
	}
	return validateID("request_id", requestID)
}

func normalizeRequestDefinition(value json.RawMessage) (string, error) {
	const maxDefinitionBytes = 1024 * 1024
	definition := bytes.TrimSpace(value)
	var object map[string]json.RawMessage
	if len(definition) == 0 || len(definition) > maxDefinitionBytes || json.Unmarshal(definition, &object) != nil || object == nil {
		return "", problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"definition": "must be a JSON object no larger than 1 MiB"},
		)
	}
	return string(definition), nil
}

func validateID(field, value string) error {
	if _, err := uuid.Parse(value); err != nil {
		return problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{field: "must be a valid UUID"},
		)
	}
	return nil
}

func validateIDs(field string, values []string) error {
	for _, value := range values {
		if err := validateID(field, value); err != nil {
			return err
		}
	}
	return nil
}

func mapRepositoryError(err error) error {
	switch {
	case errors.Is(err, ErrWorkspaceNotFound):
		return problem.New(problem.KindNotFound, "workspace_not_found", "workspace was not found")
	case errors.Is(err, ErrCollectionNotFound):
		return problem.New(problem.KindNotFound, "collection_not_found", "collection was not found")
	case errors.Is(err, ErrSavedRequestNotFound):
		return problem.New(problem.KindNotFound, "request_not_found", "saved request was not found")
	case errors.Is(err, ErrParentCollectionMissing):
		return problem.New(problem.KindNotFound, "parent_collection_not_found", "parent collection was not found in the workspace")
	case errors.Is(err, ErrUnknownUser):
		return problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"user_ids": "contains an unknown user"},
		)
	case errors.Is(err, ErrCollectionCycle):
		return problem.New(problem.KindConflict, "collection_cycle", "a collection cannot be moved into itself or one of its descendants")
	default:
		return problem.Wrap(err, "persist workspace")
	}
}

func workspaceAccessDenied() error {
	return problem.New(problem.KindForbidden, "workspace_access_denied", "the account does not have access to the workspace")
}

func collectionAccessDenied() error {
	return problem.New(problem.KindForbidden, "collection_access_denied", "the account does not have access to the collection")
}
