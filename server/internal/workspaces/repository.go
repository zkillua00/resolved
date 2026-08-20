package workspaces

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"time"

	"resolved-server/internal/identity"
	"resolved-server/internal/security"

	"gorm.io/gorm"
)

const databaseBatchSize = 200

type Repository struct {
	db         *gorm.DB
	dataCipher *security.DataCipher
}

func NewRepository(db *gorm.DB, dataCipher ...*security.DataCipher) *Repository {
	repository := &Repository{db: db}
	if len(dataCipher) > 0 {
		repository.dataCipher = dataCipher[0]
	}
	return repository
}

func (r *Repository) ListWorkspaces(ctx context.Context) ([]Workspace, error) {
	var workspaces []Workspace
	if err := r.db.WithContext(ctx).Order("created_at ASC").Order("id ASC").Find(&workspaces).Error; err != nil {
		return nil, err
	}
	for index := range workspaces {
		if err := r.decryptResourceName(ctx, r.db.WithContext(ctx), workspaces[index].ID, "workspace_name", workspaces[index].ID, workspaces[index].EncryptedName, &workspaces[index].Name); err != nil {
			return nil, err
		}
		if err := r.hydrateCreator(r.db.WithContext(ctx), workspaces[index].CreatedByUserID, &workspaces[index].CreatedByUser); err != nil {
			return nil, err
		}
		if err := r.hydrateWorkspace(r.db.WithContext(ctx), &workspaces[index]); err != nil {
			return nil, err
		}
	}
	return workspaces, nil
}

func (r *Repository) GetWorkspace(ctx context.Context, id string) (Workspace, error) {
	var workspace Workspace
	if err := r.db.WithContext(ctx).First(&workspace, "id = ?", id).Error; err != nil {
		if errors.Is(err, gorm.ErrRecordNotFound) {
			return Workspace{}, ErrWorkspaceNotFound
		}
		return Workspace{}, err
	}
	if err := r.decryptResourceName(ctx, r.db.WithContext(ctx), workspace.ID, "workspace_name", workspace.ID, workspace.EncryptedName, &workspace.Name); err != nil {
		return Workspace{}, err
	}
	if err := r.hydrateCreator(r.db.WithContext(ctx), workspace.CreatedByUserID, &workspace.CreatedByUser); err != nil {
		return Workspace{}, err
	}
	if err := r.hydrateWorkspace(r.db.WithContext(ctx), &workspace); err != nil {
		return Workspace{}, err
	}
	return workspace, nil
}

func (r *Repository) CreateWorkspace(ctx context.Context, workspace Workspace, userIDs []string) (Workspace, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		userIDs = uniqueStrings(userIDs)
		if err := ensureUsersExist(tx, userIDs); err != nil {
			return err
		}
		if err := r.encryptResourceName(ctx, tx, workspace.ID, "workspace_name", workspace.ID, &workspace.Name, &workspace.EncryptedName); err != nil {
			return err
		}
		if err := tx.Create(&workspace).Error; err != nil {
			return err
		}
		return insertWorkspaceUsers(tx, workspace.ID, userIDs)
	})
	if err != nil {
		return Workspace{}, err
	}
	return r.GetWorkspace(ctx, workspace.ID)
}

func (r *Repository) UpdateWorkspaceName(ctx context.Context, id, name string) (Workspace, error) {
	var encryptedName []byte
	if err := r.encryptResourceName(ctx, r.db.WithContext(ctx), id, "workspace_name", id, &name, &encryptedName); err != nil {
		return Workspace{}, err
	}
	updates := map[string]any{"name": name}
	if r.dataCipher != nil {
		updates = map[string]any{"name": "", "encrypted_name": encryptedName}
	}
	result := r.db.WithContext(ctx).Model(&Workspace{}).Where("id = ?", id).Updates(updates)
	if result.Error != nil {
		return Workspace{}, result.Error
	}
	if result.RowsAffected == 0 {
		return Workspace{}, ErrWorkspaceNotFound
	}
	return r.GetWorkspace(ctx, id)
}

func (r *Repository) ReplaceWorkspaceUsers(ctx context.Context, id string, userIDs []string) (Workspace, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureWorkspaceExists(tx, id); err != nil {
			return err
		}
		userIDs = uniqueStrings(userIDs)
		if err := ensureUsersExist(tx, userIDs); err != nil {
			return err
		}
		if err := tx.Where("workspace_id = ?", id).Delete(&WorkspaceUser{}).Error; err != nil {
			return err
		}
		if err := insertWorkspaceUsers(tx, id, userIDs); err != nil {
			return err
		}
		return touchWorkspace(tx, id)
	})
	if err != nil {
		return Workspace{}, err
	}
	return r.GetWorkspace(ctx, id)
}

func (r *Repository) DeleteWorkspace(ctx context.Context, id string) error {
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureWorkspaceExists(tx, id); err != nil {
			return err
		}
		collectionIDs := tx.Model(&Collection{}).Select("id").Where("workspace_id = ?", id)
		if err := tx.Where("collection_id IN (?)", collectionIDs).Delete(&SavedRequest{}).Error; err != nil {
			return err
		}
		if err := tx.Where("collection_id IN (?)", collectionIDs).Delete(&CollectionUser{}).Error; err != nil {
			return err
		}
		if err := tx.Where("workspace_id = ?", id).Delete(&Collection{}).Error; err != nil {
			return err
		}
		if err := tx.Where("workspace_id = ?", id).Delete(&WorkspaceUser{}).Error; err != nil {
			return err
		}
		environmentIDs := tx.Model(&Environment{}).Select("id").Where("workspace_id = ?", id)
		variableIDs := tx.Model(&EnvironmentVariable{}).Select("id").Where("environment_id IN (?)", environmentIDs)
		if err := tx.Where("environment_variable_id IN (?)", variableIDs).
			Delete(&EnvironmentVariableValue{}).Error; err != nil {
			return err
		}
		if err := tx.Where("environment_id IN (?)", environmentIDs).Delete(&EnvironmentVariable{}).Error; err != nil {
			return err
		}
		if err := tx.Where("workspace_id = ?", id).Delete(&Environment{}).Error; err != nil {
			return err
		}
		return tx.Delete(&Workspace{}, "id = ?", id).Error
	})
}

func (r *Repository) CreateCollection(ctx context.Context, collection Collection) (Collection, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureWorkspaceExists(tx, collection.WorkspaceID); err != nil {
			return err
		}
		if collection.ParentCollectionID != nil {
			if err := ensureCollectionExists(tx, collection.WorkspaceID, *collection.ParentCollectionID); err != nil {
				if errors.Is(err, ErrCollectionNotFound) {
					return ErrParentCollectionMissing
				}
				return err
			}
		}
		if err := r.encryptResourceName(ctx, tx, collection.WorkspaceID, "collection_name", collection.ID, &collection.Name, &collection.EncryptedName); err != nil {
			return err
		}
		if err := tx.Create(&collection).Error; err != nil {
			return err
		}
		return touchWorkspace(tx, collection.WorkspaceID)
	})
	if err != nil {
		return Collection{}, err
	}
	return r.getCollection(ctx, collection.WorkspaceID, collection.ID)
}

func (r *Repository) UpdateCollectionName(ctx context.Context, workspaceID, id, name string) (Collection, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var encryptedName []byte
		if err := r.encryptResourceName(ctx, tx, workspaceID, "collection_name", id, &name, &encryptedName); err != nil {
			return err
		}
		updates := map[string]any{"name": name}
		if r.dataCipher != nil {
			updates = map[string]any{"name": "", "encrypted_name": encryptedName}
		}
		result := tx.Model(&Collection{}).
			Where("workspace_id = ? AND id = ?", workspaceID, id).
			Updates(updates)
		if result.Error != nil {
			return result.Error
		}
		if result.RowsAffected == 0 {
			return ErrCollectionNotFound
		}
		return touchWorkspace(tx, workspaceID)
	})
	if err != nil {
		return Collection{}, err
	}
	return r.getCollection(ctx, workspaceID, id)
}

func (r *Repository) MoveCollection(
	ctx context.Context,
	workspaceID, id string,
	parentCollectionID *string,
) (Collection, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		collections, err := r.loadFlatCollections(tx, workspaceID)
		if err != nil {
			return err
		}
		byID := make(map[string]Collection, len(collections))
		for _, collection := range collections {
			byID[collection.ID] = collection
		}
		if _, exists := byID[id]; !exists {
			return ErrCollectionNotFound
		}
		if parentCollectionID != nil {
			parent, exists := byID[*parentCollectionID]
			if !exists {
				return ErrParentCollectionMissing
			}
			seen := map[string]struct{}{}
			for {
				if parent.ID == id {
					return ErrCollectionCycle
				}
				if _, exists := seen[parent.ID]; exists {
					return ErrCollectionCycle
				}
				seen[parent.ID] = struct{}{}
				if parent.ParentCollectionID == nil {
					break
				}
				next, exists := byID[*parent.ParentCollectionID]
				if !exists {
					return fmt.Errorf("collection %s references missing parent %s", parent.ID, *parent.ParentCollectionID)
				}
				parent = next
			}
		}
		if err := tx.Model(&Collection{}).
			Where("workspace_id = ? AND id = ?", workspaceID, id).
			Update("parent_collection_id", parentCollectionID).Error; err != nil {
			return err
		}
		return touchWorkspace(tx, workspaceID)
	})
	if err != nil {
		return Collection{}, err
	}
	return r.getCollection(ctx, workspaceID, id)
}

func (r *Repository) ReplaceCollectionUsers(
	ctx context.Context,
	workspaceID, id string,
	userIDs []string,
) (Collection, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureCollectionExists(tx, workspaceID, id); err != nil {
			return err
		}
		userIDs = uniqueStrings(userIDs)
		if err := ensureUsersExist(tx, userIDs); err != nil {
			return err
		}
		if err := tx.Where("collection_id = ?", id).Delete(&CollectionUser{}).Error; err != nil {
			return err
		}
		if err := insertCollectionUsers(tx, id, userIDs); err != nil {
			return err
		}
		if err := tx.Model(&Collection{}).Where("id = ?", id).UpdateColumn("updated_at", time.Now().UTC()).Error; err != nil {
			return err
		}
		return touchWorkspace(tx, workspaceID)
	})
	if err != nil {
		return Collection{}, err
	}
	return r.getCollection(ctx, workspaceID, id)
}

func (r *Repository) DeleteCollection(ctx context.Context, workspaceID, id string) error {
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		collections, err := r.loadFlatCollections(tx, workspaceID)
		if err != nil {
			return err
		}
		children := make(map[string][]string)
		found := false
		for _, collection := range collections {
			if collection.ID == id {
				found = true
			}
			if collection.ParentCollectionID != nil {
				children[*collection.ParentCollectionID] = append(children[*collection.ParentCollectionID], collection.ID)
			}
		}
		if !found {
			return ErrCollectionNotFound
		}

		ids := []string{id}
		for index := 0; index < len(ids); index++ {
			ids = append(ids, children[ids[index]]...)
		}
		for start := 0; start < len(ids); start += databaseBatchSize {
			end := min(start+databaseBatchSize, len(ids))
			if err := tx.Where("collection_id IN ?", ids[start:end]).Delete(&SavedRequest{}).Error; err != nil {
				return err
			}
		}
		for start := 0; start < len(ids); start += databaseBatchSize {
			end := min(start+databaseBatchSize, len(ids))
			if err := tx.Where("collection_id IN ?", ids[start:end]).Delete(&CollectionUser{}).Error; err != nil {
				return err
			}
		}
		for start := 0; start < len(ids); start += databaseBatchSize {
			end := min(start+databaseBatchSize, len(ids))
			if err := tx.Where("id IN ?", ids[start:end]).Delete(&Collection{}).Error; err != nil {
				return err
			}
		}
		return touchWorkspace(tx, workspaceID)
	})
}

func (r *Repository) CreateSavedRequest(
	ctx context.Context,
	workspaceID string,
	request SavedRequest,
) (SavedRequest, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureCollectionExists(tx, workspaceID, request.CollectionID); err != nil {
			return err
		}
		if err := r.encryptSavedRequest(ctx, tx, workspaceID, &request); err != nil {
			return err
		}
		if err := tx.Create(&request).Error; err != nil {
			return err
		}
		return touchWorkspace(tx, workspaceID)
	})
	if err != nil {
		return SavedRequest{}, err
	}
	return r.GetSavedRequest(ctx, workspaceID, request.CollectionID, request.ID)
}

func (r *Repository) GetSavedRequest(
	ctx context.Context,
	workspaceID, collectionID, requestID string,
) (SavedRequest, error) {
	var request SavedRequest
	err := r.db.WithContext(ctx).
		Model(&SavedRequest{}).
		Joins("JOIN collections ON collections.id = saved_requests.collection_id").
		Where(
			"collections.workspace_id = ? AND saved_requests.collection_id = ? AND saved_requests.id = ?",
			workspaceID,
			collectionID,
			requestID,
		).
		Select("saved_requests.*").
		First(&request).Error
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return SavedRequest{}, ErrSavedRequestNotFound
	}
	if err == nil {
		err = r.decryptSavedRequest(ctx, r.db.WithContext(ctx), workspaceID, &request)
	}
	if err == nil {
		err = r.hydrateCreator(r.db.WithContext(ctx), request.CreatedByUserID, &request.CreatedByUser)
	}
	return request, err
}

func (r *Repository) UpdateSavedRequest(
	ctx context.Context,
	workspaceID, collectionID, requestID, name, definition string,
) (SavedRequest, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		request := SavedRequest{ID: requestID, CollectionID: collectionID, Name: name, Definition: definition}
		if err := r.encryptSavedRequest(ctx, tx, workspaceID, &request); err != nil {
			return err
		}
		updates := map[string]any{"name": name, "definition": definition}
		if r.dataCipher != nil {
			updates = map[string]any{
				"name": "", "definition": "", "encrypted_payload": request.EncryptedPayload,
			}
		}
		result := tx.Model(&SavedRequest{}).
			Where("collection_id = ? AND id = ?", collectionID, requestID).
			Updates(updates)
		if result.Error != nil {
			return result.Error
		}
		if result.RowsAffected == 0 {
			return ErrSavedRequestNotFound
		}
		if err := ensureCollectionExists(tx, workspaceID, collectionID); err != nil {
			return err
		}
		return touchWorkspace(tx, workspaceID)
	})
	if err != nil {
		return SavedRequest{}, err
	}
	return r.GetSavedRequest(ctx, workspaceID, collectionID, requestID)
}

func (r *Repository) MoveSavedRequest(
	ctx context.Context,
	workspaceID, collectionID, requestID, targetCollectionID string,
) (SavedRequest, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureCollectionExists(tx, workspaceID, collectionID); err != nil {
			return err
		}
		if err := ensureCollectionExists(tx, workspaceID, targetCollectionID); err != nil {
			return err
		}
		result := tx.Model(&SavedRequest{}).
			Where("collection_id = ? AND id = ?", collectionID, requestID).
			Update("collection_id", targetCollectionID)
		if result.Error != nil {
			return result.Error
		}
		if result.RowsAffected == 0 {
			return ErrSavedRequestNotFound
		}
		return touchWorkspace(tx, workspaceID)
	})
	if err != nil {
		return SavedRequest{}, err
	}
	return r.GetSavedRequest(ctx, workspaceID, targetCollectionID, requestID)
}

func (r *Repository) DeleteSavedRequest(
	ctx context.Context,
	workspaceID, collectionID, requestID string,
) error {
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureCollectionExists(tx, workspaceID, collectionID); err != nil {
			return err
		}
		result := tx.Where("collection_id = ? AND id = ?", collectionID, requestID).
			Delete(&SavedRequest{})
		if result.Error != nil {
			return result.Error
		}
		if result.RowsAffected == 0 {
			return ErrSavedRequestNotFound
		}
		return touchWorkspace(tx, workspaceID)
	})
}

func (r *Repository) getCollection(ctx context.Context, workspaceID, id string) (Collection, error) {
	workspace, err := r.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return Collection{}, err
	}
	collection, found := findCollection(workspace.Collections, id)
	if !found {
		return Collection{}, ErrCollectionNotFound
	}
	return collection, nil
}

func (r *Repository) hydrateWorkspace(db *gorm.DB, workspace *Workspace) error {
	var workspaceUsers []WorkspaceUser
	if err := db.Where("workspace_id = ?", workspace.ID).Order("user_id ASC").Find(&workspaceUsers).Error; err != nil {
		return err
	}
	workspace.UserIDs = make([]string, 0, len(workspaceUsers))
	for _, grant := range workspaceUsers {
		workspace.UserIDs = append(workspace.UserIDs, grant.UserID)
	}

	collections, err := r.loadFlatCollections(db, workspace.ID)
	if err != nil {
		return err
	}
	if len(collections) == 0 {
		workspace.Collections = []Collection{}
		return nil
	}
	var collectionUsers []CollectionUser
	if err := db.Model(&CollectionUser{}).
		Select("collection_users.*").
		Joins("JOIN collections ON collections.id = collection_users.collection_id").
		Where("collections.workspace_id = ?", workspace.ID).
		Order("collection_id ASC").Order("user_id ASC").
		Find(&collectionUsers).Error; err != nil {
		return err
	}
	usersByCollection := make(map[string][]string)
	for _, grant := range collectionUsers {
		usersByCollection[grant.CollectionID] = append(usersByCollection[grant.CollectionID], grant.UserID)
	}
	for index := range collections {
		collections[index].UserIDs = cloneStrings(usersByCollection[collections[index].ID])
		collections[index].SubCollections = []Collection{}
		collections[index].Requests = []SavedRequest{}
	}
	requests, err := r.loadSavedRequests(db, workspace.ID)
	if err != nil {
		return err
	}
	requestsByCollection := make(map[string][]SavedRequest)
	for _, request := range requests {
		requestsByCollection[request.CollectionID] = append(requestsByCollection[request.CollectionID], request)
	}
	for index := range collections {
		collections[index].Requests = append(
			collections[index].Requests,
			requestsByCollection[collections[index].ID]...,
		)
	}
	tree, err := buildCollectionTree(collections)
	if err != nil {
		return err
	}
	workspace.Collections = tree
	return nil
}

func (r *Repository) loadSavedRequests(db *gorm.DB, workspaceID string) ([]SavedRequest, error) {
	var requests []SavedRequest
	err := db.Model(&SavedRequest{}).
		Select("saved_requests.*").
		Joins("JOIN collections ON collections.id = saved_requests.collection_id").
		Where("collections.workspace_id = ?", workspaceID).
		Order("saved_requests.created_at ASC").
		Order("saved_requests.id ASC").
		Find(&requests).Error
	if err == nil {
		for index := range requests {
			if decryptErr := r.decryptSavedRequest(db.Statement.Context, db, workspaceID, &requests[index]); decryptErr != nil {
				return nil, decryptErr
			}
			if hydrateErr := r.hydrateCreator(db, requests[index].CreatedByUserID, &requests[index].CreatedByUser); hydrateErr != nil {
				return nil, hydrateErr
			}
		}
	}
	return requests, err
}

type savedRequestPayload struct {
	Name       string `json:"name"`
	Definition string `json:"definition"`
}

func (r *Repository) encryptSavedRequest(
	ctx context.Context,
	db *gorm.DB,
	workspaceID string,
	request *SavedRequest,
) error {
	if r.dataCipher == nil {
		return nil
	}
	plaintext, err := json.Marshal(savedRequestPayload{Name: request.Name, Definition: request.Definition})
	if err != nil {
		return fmt.Errorf("encode saved request encryption payload: %w", err)
	}
	defer clear(plaintext)
	encrypted, err := r.dataCipher.Encrypt(
		ctx, db, security.WorkspaceDataScope(workspaceID), "saved_request", request.ID, plaintext,
	)
	if err != nil {
		return fmt.Errorf("encrypt saved request: %w", err)
	}
	request.EncryptedPayload = encrypted
	request.Name = ""
	request.Definition = ""
	return nil
}

func (r *Repository) decryptSavedRequest(
	ctx context.Context,
	db *gorm.DB,
	workspaceID string,
	request *SavedRequest,
) error {
	if len(request.EncryptedPayload) == 0 {
		return nil
	}
	if r.dataCipher == nil {
		return security.ErrDataKeyUnavailable
	}
	plaintext, err := r.dataCipher.Decrypt(
		ctx, db, security.WorkspaceDataScope(workspaceID), "saved_request", request.ID, request.EncryptedPayload,
	)
	if err != nil {
		return fmt.Errorf("decrypt saved request: %w", err)
	}
	defer clear(plaintext)
	var payload savedRequestPayload
	if err := json.Unmarshal(plaintext, &payload); err != nil {
		return fmt.Errorf("decode saved request encryption payload: %w", err)
	}
	request.Name = payload.Name
	request.Definition = payload.Definition
	return nil
}

func (r *Repository) EncryptLegacySavedRequests(ctx context.Context) error {
	if r.dataCipher == nil {
		return security.ErrDataKeyUnavailable
	}
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var rows []struct {
			ID           string
			CollectionID string
			WorkspaceID  string
			Name         string
			Definition   string
		}
		if err := tx.Table("saved_requests").
			Select("saved_requests.id, saved_requests.collection_id, collections.workspace_id, saved_requests.name, saved_requests.definition").
			Joins("JOIN collections ON collections.id = saved_requests.collection_id").
			Where("saved_requests.encrypted_payload IS NULL").
			Scan(&rows).Error; err != nil {
			return err
		}
		for _, row := range rows {
			request := SavedRequest{
				ID: row.ID, CollectionID: row.CollectionID, Name: row.Name, Definition: row.Definition,
			}
			if err := r.encryptSavedRequest(ctx, tx, row.WorkspaceID, &request); err != nil {
				return err
			}
			if err := tx.Model(&SavedRequest{}).Where("id = ?", row.ID).Updates(map[string]any{
				"name": "", "definition": "", "encrypted_payload": request.EncryptedPayload,
			}).Error; err != nil {
				return err
			}
		}
		return nil
	})
}

func (r *Repository) encryptResourceName(
	ctx context.Context,
	db *gorm.DB,
	workspaceID, domain, recordID string,
	name *string,
	encryptedName *[]byte,
) error {
	if r.dataCipher == nil {
		return nil
	}
	encrypted, err := r.dataCipher.Encrypt(
		ctx, db, security.WorkspaceDataScope(workspaceID), domain, recordID, []byte(*name),
	)
	if err != nil {
		return fmt.Errorf("encrypt %s: %w", domain, err)
	}
	*encryptedName = encrypted
	*name = ""
	return nil
}

func (r *Repository) decryptResourceName(
	ctx context.Context,
	db *gorm.DB,
	workspaceID, domain, recordID string,
	encryptedName []byte,
	name *string,
) error {
	if len(encryptedName) == 0 {
		return nil
	}
	if r.dataCipher == nil {
		return security.ErrDataKeyUnavailable
	}
	plaintext, err := r.dataCipher.Decrypt(
		ctx, db, security.WorkspaceDataScope(workspaceID), domain, recordID, encryptedName,
	)
	if err != nil {
		return fmt.Errorf("decrypt %s: %w", domain, err)
	}
	defer clear(plaintext)
	*name = string(plaintext)
	return nil
}

func (r *Repository) EncryptLegacyResourceNames(ctx context.Context) error {
	if r.dataCipher == nil {
		return security.ErrDataKeyUnavailable
	}
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var workspaceRows []Workspace
		if err := tx.Where("encrypted_name IS NULL").Find(&workspaceRows).Error; err != nil {
			return err
		}
		for index := range workspaceRows {
			if err := r.encryptResourceName(
				ctx, tx, workspaceRows[index].ID, "workspace_name", workspaceRows[index].ID,
				&workspaceRows[index].Name, &workspaceRows[index].EncryptedName,
			); err != nil {
				return err
			}
			if err := tx.Model(&Workspace{}).Where("id = ?", workspaceRows[index].ID).Updates(map[string]any{
				"name": "", "encrypted_name": workspaceRows[index].EncryptedName,
			}).Error; err != nil {
				return err
			}
		}

		var collectionRows []Collection
		if err := tx.Where("encrypted_name IS NULL").Find(&collectionRows).Error; err != nil {
			return err
		}
		for index := range collectionRows {
			if err := r.encryptResourceName(
				ctx, tx, collectionRows[index].WorkspaceID, "collection_name", collectionRows[index].ID,
				&collectionRows[index].Name, &collectionRows[index].EncryptedName,
			); err != nil {
				return err
			}
			if err := tx.Model(&Collection{}).Where("id = ?", collectionRows[index].ID).Updates(map[string]any{
				"name": "", "encrypted_name": collectionRows[index].EncryptedName,
			}).Error; err != nil {
				return err
			}
		}

		var environmentRows []Environment
		if err := tx.Where("encrypted_name IS NULL").Find(&environmentRows).Error; err != nil {
			return err
		}
		for index := range environmentRows {
			if err := r.encryptResourceName(
				ctx, tx, environmentRows[index].WorkspaceID, "environment_name", environmentRows[index].ID,
				&environmentRows[index].Name, &environmentRows[index].EncryptedName,
			); err != nil {
				return err
			}
			if err := tx.Model(&Environment{}).Where("id = ?", environmentRows[index].ID).Updates(map[string]any{
				"name": "", "encrypted_name": environmentRows[index].EncryptedName,
			}).Error; err != nil {
				return err
			}
		}
		return nil
	})
}

func (r *Repository) loadFlatCollections(db *gorm.DB, workspaceID string) ([]Collection, error) {
	var collections []Collection
	err := db.Where("workspace_id = ?", workspaceID).
		Order("created_at ASC").Order("id ASC").
		Find(&collections).Error
	if err == nil {
		for index := range collections {
			if decryptErr := r.decryptResourceName(db.Statement.Context, db, workspaceID, "collection_name", collections[index].ID, collections[index].EncryptedName, &collections[index].Name); decryptErr != nil {
				return nil, decryptErr
			}
			if hydrateErr := r.hydrateCreator(db, collections[index].CreatedByUserID, &collections[index].CreatedByUser); hydrateErr != nil {
				return nil, hydrateErr
			}
		}
	}
	return collections, err
}

func (r *Repository) hydrateCreator(db *gorm.DB, id *string, target **identity.User) error {
	if id == nil {
		*target = nil
		return nil
	}
	var creator identity.User
	if err := db.Select("id", "email", "display_name", "encrypted_profile").First(&creator, "id = ?", *id).Error; err != nil {
		return err
	}
	if err := identity.DecryptUserProfile(db.Statement.Context, db, r.dataCipher, &creator); err != nil {
		return err
	}
	*target = &creator
	return nil
}

func buildCollectionTree(collections []Collection) ([]Collection, error) {
	byID := make(map[string]Collection, len(collections))
	children := make(map[string][]string)
	rootIDs := make([]string, 0)
	for _, collection := range collections {
		byID[collection.ID] = collection
	}
	for _, collection := range collections {
		if collection.ParentCollectionID == nil {
			rootIDs = append(rootIDs, collection.ID)
			continue
		}
		if _, exists := byID[*collection.ParentCollectionID]; !exists {
			return nil, fmt.Errorf("collection %s references missing parent %s", collection.ID, *collection.ParentCollectionID)
		}
		children[*collection.ParentCollectionID] = append(children[*collection.ParentCollectionID], collection.ID)
	}

	state := make(map[string]uint8, len(collections))
	var build func(string) (Collection, error)
	build = func(id string) (Collection, error) {
		if state[id] == 1 {
			return Collection{}, ErrCollectionCycle
		}
		state[id] = 1
		collection := byID[id]
		collection.SubCollections = make([]Collection, 0, len(children[id]))
		for _, childID := range children[id] {
			child, err := build(childID)
			if err != nil {
				return Collection{}, err
			}
			collection.SubCollections = append(collection.SubCollections, child)
		}
		state[id] = 2
		return collection, nil
	}

	roots := make([]Collection, 0, len(rootIDs))
	for _, id := range rootIDs {
		root, err := build(id)
		if err != nil {
			return nil, err
		}
		roots = append(roots, root)
	}
	for id := range byID {
		if state[id] == 0 {
			if _, err := build(id); err != nil {
				return nil, err
			}
			return nil, fmt.Errorf("collection %s is not connected to a workspace root", id)
		}
	}
	return roots, nil
}

func findCollection(collections []Collection, id string) (Collection, bool) {
	for _, collection := range collections {
		if collection.ID == id {
			return collection, true
		}
		if found, ok := findCollection(collection.SubCollections, id); ok {
			return found, true
		}
	}
	return Collection{}, false
}

func ensureWorkspaceExists(tx *gorm.DB, id string) error {
	var count int64
	if err := tx.Model(&Workspace{}).Where("id = ?", id).Count(&count).Error; err != nil {
		return err
	}
	if count == 0 {
		return ErrWorkspaceNotFound
	}
	return nil
}

func ensureCollectionExists(tx *gorm.DB, workspaceID, id string) error {
	var count int64
	if err := tx.Model(&Collection{}).
		Where("workspace_id = ? AND id = ?", workspaceID, id).
		Count(&count).Error; err != nil {
		return err
	}
	if count == 0 {
		return ErrCollectionNotFound
	}
	return nil
}

func ensureUsersExist(tx *gorm.DB, userIDs []string) error {
	if len(userIDs) == 0 {
		return nil
	}
	var count int64
	for start := 0; start < len(userIDs); start += databaseBatchSize {
		end := min(start+databaseBatchSize, len(userIDs))
		var batchCount int64
		if err := tx.Model(&identity.User{}).Where("id IN ?", userIDs[start:end]).Count(&batchCount).Error; err != nil {
			return err
		}
		count += batchCount
	}
	if count != int64(len(userIDs)) {
		return ErrUnknownUser
	}
	return nil
}

func insertWorkspaceUsers(tx *gorm.DB, workspaceID string, userIDs []string) error {
	grants := make([]WorkspaceUser, 0, len(userIDs))
	for _, userID := range userIDs {
		grants = append(grants, WorkspaceUser{WorkspaceID: workspaceID, UserID: userID})
	}
	if len(grants) == 0 {
		return nil
	}
	return tx.CreateInBatches(&grants, databaseBatchSize).Error
}

func insertCollectionUsers(tx *gorm.DB, collectionID string, userIDs []string) error {
	grants := make([]CollectionUser, 0, len(userIDs))
	for _, userID := range userIDs {
		grants = append(grants, CollectionUser{CollectionID: collectionID, UserID: userID})
	}
	if len(grants) == 0 {
		return nil
	}
	return tx.CreateInBatches(&grants, databaseBatchSize).Error
}

func touchWorkspace(tx *gorm.DB, id string) error {
	return tx.Model(&Workspace{}).Where("id = ?", id).UpdateColumn("updated_at", time.Now().UTC()).Error
}

func uniqueStrings(values []string) []string {
	unique := make(map[string]struct{}, len(values))
	for _, value := range values {
		unique[value] = struct{}{}
	}
	result := make([]string, 0, len(unique))
	for value := range unique {
		result = append(result, value)
	}
	sort.Strings(result)
	return result
}
