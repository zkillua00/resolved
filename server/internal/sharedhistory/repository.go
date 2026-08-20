package sharedhistory

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sort"

	"resolved-server/internal/identity"
	"resolved-server/internal/security"

	"gorm.io/gorm"
	"gorm.io/gorm/clause"
)

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

func (r *Repository) ListProfiles(ctx context.Context) ([]identity.User, error) {
	var users []identity.User
	err := r.db.WithContext(ctx).Order("id ASC").Find(&users).Error
	if err == nil {
		for index := range users {
			if err := identity.DecryptUserProfile(ctx, r.db.WithContext(ctx), r.dataCipher, &users[index]); err != nil {
				return nil, err
			}
		}
		sort.Slice(users, func(left, right int) bool {
			if users[left].DisplayName == users[right].DisplayName {
				return users[left].Email < users[right].Email
			}
			return users[left].DisplayName < users[right].DisplayName
		})
	}
	return users, err
}

func (r *Repository) GetProfile(ctx context.Context, userID string) (identity.User, error) {
	var user identity.User
	err := r.db.WithContext(ctx).First(&user, "id = ?", userID).Error
	if err == nil {
		err = identity.DecryptUserProfile(ctx, r.db.WithContext(ctx), r.dataCipher, &user)
	}
	return user, err
}

func (r *Repository) ListRealtimeViewerUserIDs(
	ctx context.Context,
	workspaceID, historyOwnerID string,
) ([]string, error) {
	var userIDs []string
	err := r.db.WithContext(ctx).Raw(`
		SELECT DISTINCT users.id
		FROM users
		WHERE users.active = ?
		  AND (
			users.id = ?
			OR (
			  EXISTS (
				SELECT 1
				FROM user_roles
				LEFT JOIN role_permissions
				  ON role_permissions.role_id = user_roles.role_id
				WHERE user_roles.user_id = users.id
				  AND (user_roles.role_id = ? OR role_permissions.permission_key = ?)
			  )
			  AND (
				EXISTS (
				  SELECT 1 FROM user_roles
				  WHERE user_roles.user_id = users.id AND user_roles.role_id = ?
				)
				OR EXISTS (
				  SELECT 1 FROM workspace_users
				  WHERE workspace_users.user_id = users.id
					AND workspace_users.workspace_id = ?
				)
				OR EXISTS (
				  SELECT 1
				  FROM collection_users
				  JOIN collections ON collections.id = collection_users.collection_id
				  WHERE collection_users.user_id = users.id
					AND collections.workspace_id = ?
				)
			  )
			)
		  )
		ORDER BY users.id`,
		true,
		historyOwnerID,
		identity.OwnerRoleID,
		identity.PermissionHistoryReadOthers,
		identity.OwnerRoleID,
		workspaceID,
		workspaceID,
	).Scan(&userIDs).Error
	return userIDs, err
}

func (r *Repository) UpsertEntry(ctx context.Context, entry Entry) (Entry, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := r.encryptEntry(ctx, tx, &entry); err != nil {
			return err
		}
		if r.dataCipher == nil {
			var existing Entry
			err := tx.Where(
				"workspace_id = ? AND user_id = ? AND client_entry_id = ?",
				entry.WorkspaceID, entry.UserID, entry.ClientEntryID,
			).First(&existing).Error
			switch {
			case err == nil:
				entry.ID = existing.ID
				if err := tx.Save(&entry).Error; err != nil {
					return err
				}
			case errors.Is(err, gorm.ErrRecordNotFound):
				if err := tx.Create(&entry).Error; err != nil {
					return err
				}
			default:
				return err
			}
		} else {
			if err := tx.Clauses(clause.OnConflict{
				Columns: []clause.Column{
					{Name: "workspace_id"}, {Name: "user_id"}, {Name: "client_entry_lookup"},
				},
				DoUpdates: clause.AssignmentColumns([]string{
					"encrypted_payload", "created_at", "updated_at",
				}),
			}).Create(&entry).Error; err != nil {
				return err
			}
		}

		var staleIDs []string
		if err := tx.Model(&Entry{}).
			Where("workspace_id = ? AND user_id = ?", entry.WorkspaceID, entry.UserID).
			Order("created_at DESC, id DESC").
			Offset(MaxEntriesPerProfileWorkspace).
			Pluck("id", &staleIDs).Error; err != nil {
			return err
		}
		if len(staleIDs) > 0 {
			if err := tx.Where("id IN ?", staleIDs).Delete(&Entry{}).Error; err != nil {
				return err
			}
		}
		var stored Entry
		query := tx.Where(
			"workspace_id = ? AND user_id = ? AND client_entry_lookup = ?",
			entry.WorkspaceID, entry.UserID, entry.ClientEntryLookup,
		)
		if r.dataCipher == nil {
			query = tx.Where(
				"workspace_id = ? AND user_id = ? AND client_entry_id = ?",
				entry.WorkspaceID, entry.UserID, entry.ClientEntryID,
			)
		}
		if err := query.First(&stored).Error; err != nil {
			return err
		}
		entry = stored
		return nil
	})
	if err == nil {
		err = r.decryptEntry(ctx, r.db.WithContext(ctx), &entry)
	}
	return entry, err
}

func (r *Repository) ListEntries(ctx context.Context, workspaceID, userID string) ([]Entry, error) {
	var entries []Entry
	err := r.db.WithContext(ctx).
		Where("workspace_id = ? AND user_id = ?", workspaceID, userID).
		Order("created_at DESC, id DESC").
		Limit(MaxListedEntries).
		Find(&entries).Error
	if err == nil {
		for index := range entries {
			if err := r.decryptEntry(ctx, r.db.WithContext(ctx), &entries[index]); err != nil {
				return nil, err
			}
		}
	}
	return entries, err
}

func (r *Repository) DeleteEntries(ctx context.Context, workspaceID, userID string) error {
	return r.db.WithContext(ctx).
		Where("workspace_id = ? AND user_id = ?", workspaceID, userID).
		Delete(&Entry{}).Error
}

type encryptedEntryPayload struct {
	ClientEntryID          string `json:"client_entry_id"`
	Method                 string `json:"method"`
	URL                    string `json:"url"`
	RequestHeadersJSON     []byte `json:"request_headers_json"`
	RequestBody            []byte `json:"request_body"`
	RequestBodyMode        string `json:"request_body_mode"`
	RequestBodyLanguage    string `json:"request_body_language"`
	RequestBodyFieldsJSON  []byte `json:"request_body_fields_json"`
	RequestBodyTruncated   bool   `json:"request_body_truncated"`
	ResponseStatus         *int   `json:"response_status"`
	ResponseStatusText     string `json:"response_status_text"`
	ResponseHTTPVersion    string `json:"response_http_version"`
	ResponseFinalURL       string `json:"response_final_url"`
	ResponseHeadersJSON    []byte `json:"response_headers_json"`
	ResponseBody           []byte `json:"response_body"`
	ResponseBodyTruncated  bool   `json:"response_body_truncated"`
	ResponseContentType    string `json:"response_content_type"`
	ResponseDurationMicros *int64 `json:"response_duration_micros"`
	Error                  string `json:"error"`
}

func entryPayload(entry Entry) encryptedEntryPayload {
	return encryptedEntryPayload{
		ClientEntryID: entry.ClientEntryID, Method: entry.Method, URL: entry.URL,
		RequestHeadersJSON: entry.RequestHeadersJSON, RequestBody: entry.RequestBody,
		RequestBodyMode: entry.RequestBodyMode, RequestBodyLanguage: entry.RequestBodyLanguage,
		RequestBodyFieldsJSON: entry.RequestBodyFieldsJSON,
		RequestBodyTruncated:  entry.RequestBodyTruncated,
		ResponseStatus:        entry.ResponseStatus, ResponseStatusText: entry.ResponseStatusText,
		ResponseHTTPVersion: entry.ResponseHTTPVersion, ResponseFinalURL: entry.ResponseFinalURL,
		ResponseHeadersJSON: entry.ResponseHeadersJSON, ResponseBody: entry.ResponseBody,
		ResponseBodyTruncated: entry.ResponseBodyTruncated, ResponseContentType: entry.ResponseContentType,
		ResponseDurationMicros: entry.ResponseDurationMicros, Error: entry.Error,
	}
}

func applyEntryPayload(entry *Entry, payload encryptedEntryPayload) {
	entry.ClientEntryID = payload.ClientEntryID
	entry.Method = payload.Method
	entry.URL = payload.URL
	entry.RequestHeadersJSON = payload.RequestHeadersJSON
	entry.RequestBody = payload.RequestBody
	entry.RequestBodyMode = payload.RequestBodyMode
	entry.RequestBodyLanguage = payload.RequestBodyLanguage
	entry.RequestBodyFieldsJSON = payload.RequestBodyFieldsJSON
	entry.RequestBodyTruncated = payload.RequestBodyTruncated
	entry.ResponseStatus = payload.ResponseStatus
	entry.ResponseStatusText = payload.ResponseStatusText
	entry.ResponseHTTPVersion = payload.ResponseHTTPVersion
	entry.ResponseFinalURL = payload.ResponseFinalURL
	entry.ResponseHeadersJSON = payload.ResponseHeadersJSON
	entry.ResponseBody = payload.ResponseBody
	entry.ResponseBodyTruncated = payload.ResponseBodyTruncated
	entry.ResponseContentType = payload.ResponseContentType
	entry.ResponseDurationMicros = payload.ResponseDurationMicros
	entry.Error = payload.Error
}

func clearEntryPlaintext(entry *Entry) {
	entry.ClientEntryID = ""
	entry.Method = ""
	entry.URL = ""
	entry.RequestHeadersJSON = []byte("[]")
	entry.RequestBody = []byte{}
	entry.RequestBodyMode = ""
	entry.RequestBodyLanguage = ""
	entry.RequestBodyFieldsJSON = []byte("[]")
	entry.RequestBodyTruncated = false
	entry.ResponseStatus = nil
	entry.ResponseStatusText = ""
	entry.ResponseHTTPVersion = ""
	entry.ResponseFinalURL = ""
	entry.ResponseHeadersJSON = []byte("[]")
	entry.ResponseBody = nil
	entry.ResponseBodyTruncated = false
	entry.ResponseContentType = ""
	entry.ResponseDurationMicros = nil
	entry.Error = ""
}

func (r *Repository) encryptEntry(ctx context.Context, db *gorm.DB, entry *Entry) error {
	if r.dataCipher == nil {
		return nil
	}
	lookup, err := r.dataCipher.LookupDigest(
		ctx,
		db,
		security.WorkspaceDataScope(entry.WorkspaceID),
		"shared_history_client_entry:"+entry.UserID,
		entry.ClientEntryID,
	)
	if err != nil {
		return fmt.Errorf("compute shared history client-entry lookup: %w", err)
	}
	plaintext, err := json.Marshal(entryPayload(*entry))
	if err != nil {
		return fmt.Errorf("encode shared history encryption payload: %w", err)
	}
	defer clear(plaintext)
	entry.EncryptedPayload, err = r.dataCipher.Encrypt(
		ctx, db, security.WorkspaceDataScope(entry.WorkspaceID), "shared_history", entry.ID, plaintext,
	)
	if err != nil {
		return fmt.Errorf("encrypt shared history: %w", err)
	}
	entry.ClientEntryLookup = &lookup
	clearEntryPlaintext(entry)
	return nil
}

func (r *Repository) decryptEntry(ctx context.Context, db *gorm.DB, entry *Entry) error {
	if len(entry.EncryptedPayload) == 0 {
		return nil
	}
	if r.dataCipher == nil {
		return security.ErrDataKeyUnavailable
	}
	plaintext, err := r.dataCipher.Decrypt(
		ctx, db, security.WorkspaceDataScope(entry.WorkspaceID), "shared_history", entry.ID, entry.EncryptedPayload,
	)
	if err != nil {
		return fmt.Errorf("decrypt shared history: %w", err)
	}
	defer clear(plaintext)
	var payload encryptedEntryPayload
	if err := json.Unmarshal(plaintext, &payload); err != nil {
		return fmt.Errorf("decode shared history encryption payload: %w", err)
	}
	applyEntryPayload(entry, payload)
	return nil
}

func (r *Repository) EncryptLegacyEntries(ctx context.Context) error {
	if r.dataCipher == nil {
		return security.ErrDataKeyUnavailable
	}
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var entries []Entry
		if err := tx.Where("encrypted_payload IS NULL").Find(&entries).Error; err != nil {
			return err
		}
		for index := range entries {
			if err := r.encryptEntry(ctx, tx, &entries[index]); err != nil {
				return err
			}
			if err := tx.Model(&Entry{}).Where("id = ?", entries[index].ID).Updates(map[string]any{
				"client_entry_id": "", "client_entry_lookup": entries[index].ClientEntryLookup,
				"method": "", "url": "", "request_headers_json": []byte("[]"),
				"request_body": []byte{}, "request_body_mode": "", "request_body_language": "",
				"request_body_fields_json": []byte("[]"), "request_body_truncated": false,
				"response_status": nil, "response_status_text": "", "response_http_version": "",
				"response_final_url": "", "response_headers_json": []byte("[]"), "response_body": nil,
				"response_body_truncated": false, "response_content_type": "",
				"response_duration_micros": nil, "error": "", "encrypted_payload": entries[index].EncryptedPayload,
			}).Error; err != nil {
				return err
			}
		}
		return nil
	})
}
