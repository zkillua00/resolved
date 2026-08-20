package activitylog

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"

	"resolved-server/internal/identity"
	"resolved-server/internal/security"

	"gorm.io/gorm"
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

func (r *Repository) Create(ctx context.Context, entry Entry) error {
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if entry.ActorUserID != "" {
			var actor identity.User
			if err := tx.Select("id", "email", "display_name", "encrypted_profile").First(&actor, "id = ?", entry.ActorUserID).Error; err == nil {
				if err := identity.DecryptUserProfile(ctx, tx, r.dataCipher, &actor); err != nil {
					return err
				}
				entry.ActorEmail = actor.Email
				entry.ActorDisplayName = actor.DisplayName
			} else if err != nil && !errors.Is(err, gorm.ErrRecordNotFound) {
				return err
			}
		}
		if err := r.encryptEntry(ctx, tx, &entry); err != nil {
			return err
		}
		if err := tx.Create(&entry).Error; err != nil {
			return err
		}
		query := tx.Model(&Entry{}).Select("id").Order("created_at DESC, id DESC")
		limit := MaxAuditEntries
		if entry.Kind == KindChange {
			query = query.Where("kind = ? AND workspace_id = ?", KindChange, entry.WorkspaceID)
			limit = MaxWorkspaceEntries
		} else {
			query = query.Where("kind = ?", KindAudit)
		}
		var staleIDs []string
		if err := query.Offset(limit).Pluck("id", &staleIDs).Error; err != nil {
			return err
		}
		if len(staleIDs) > 0 {
			return tx.Where("id IN ?", staleIDs).Delete(&Entry{}).Error
		}
		return nil
	})
}

func (r *Repository) ListWorkspace(
	ctx context.Context,
	workspaceID string,
	allCollections bool,
	collectionIDs []string,
	page pageQuery,
) (entryPage, error) {
	query := r.db.WithContext(ctx).
		Where("kind = ? AND workspace_id = ?", KindChange, workspaceID)
	if !allCollections {
		if len(collectionIDs) == 0 {
			return entryPage{Entries: []Entry{}}, nil
		}
		query = query.Where("collection_id IN ?", collectionIDs)
	}
	return r.listPage(ctx, query, page)
}

func (r *Repository) ListAudit(ctx context.Context, page pageQuery) (entryPage, error) {
	query := r.db.WithContext(ctx).Where("kind = ?", KindAudit)
	return r.listPage(ctx, query, page)
}

func (r *Repository) listPage(ctx context.Context, query *gorm.DB, page pageQuery) (entryPage, error) {
	order := "created_at DESC, id DESC"
	if page.Before != nil {
		query = query.Where(
			"created_at < ? OR (created_at = ? AND id < ?)",
			page.Before.CreatedAt,
			page.Before.CreatedAt,
			page.Before.ID,
		)
	}
	if page.After != nil {
		query = query.Where(
			"created_at > ? OR (created_at = ? AND id > ?)",
			page.After.CreatedAt,
			page.After.CreatedAt,
			page.After.ID,
		)
		order = "created_at ASC, id ASC"
	}
	var entries []Entry
	if err := query.Order(order).Limit(page.Limit + 1).Find(&entries).Error; err != nil {
		return entryPage{}, err
	}
	hasMore := len(entries) > page.Limit
	if hasMore {
		entries = entries[:page.Limit]
	}
	for index := range entries {
		if err := r.decryptEntry(ctx, query, &entries[index]); err != nil {
			return entryPage{}, err
		}
	}
	return entryPage{Entries: entries, HasMore: hasMore}, nil
}

type encryptedActivityPayload struct {
	ActorEmail       string `json:"actor_email"`
	ActorDisplayName string `json:"actor_display_name"`
	TargetName       string `json:"target_name"`
	DiffsJSON        []byte `json:"diffs_json"`
}

func (r *Repository) encryptEntry(ctx context.Context, db *gorm.DB, entry *Entry) error {
	if r.dataCipher == nil {
		return nil
	}
	payload := encryptedActivityPayload{
		ActorEmail: entry.ActorEmail, ActorDisplayName: entry.ActorDisplayName,
		TargetName: entry.TargetName, DiffsJSON: entry.DiffsJSON,
	}
	plaintext, err := json.Marshal(payload)
	if err != nil {
		return fmt.Errorf("encode activity log encryption payload: %w", err)
	}
	defer clear(plaintext)
	scope := security.DeploymentDataScope()
	if entry.Kind == KindChange && entry.WorkspaceID != "" {
		scope = security.WorkspaceDataScope(entry.WorkspaceID)
	}
	entry.EncryptedPayload, err = r.dataCipher.Encrypt(ctx, db, scope, "activity_log", entry.ID, plaintext)
	if err != nil {
		return fmt.Errorf("encrypt activity log: %w", err)
	}
	entry.ActorEmail = ""
	entry.ActorDisplayName = ""
	entry.TargetName = ""
	entry.DiffsJSON = []byte("[]")
	return nil
}

func (r *Repository) decryptEntry(ctx context.Context, db *gorm.DB, entry *Entry) error {
	if len(entry.EncryptedPayload) == 0 {
		return nil
	}
	if r.dataCipher == nil {
		return security.ErrDataKeyUnavailable
	}
	scope := security.DeploymentDataScope()
	if entry.Kind == KindChange && entry.WorkspaceID != "" {
		scope = security.WorkspaceDataScope(entry.WorkspaceID)
	}
	plaintext, err := r.dataCipher.Decrypt(ctx, db, scope, "activity_log", entry.ID, entry.EncryptedPayload)
	if err != nil {
		return fmt.Errorf("decrypt activity log: %w", err)
	}
	defer clear(plaintext)
	var payload encryptedActivityPayload
	if err := json.Unmarshal(plaintext, &payload); err != nil {
		return fmt.Errorf("decode activity log encryption payload: %w", err)
	}
	entry.ActorEmail = payload.ActorEmail
	entry.ActorDisplayName = payload.ActorDisplayName
	entry.TargetName = payload.TargetName
	entry.DiffsJSON = payload.DiffsJSON
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
				"actor_email": "", "actor_display_name": "", "target_name": "",
				"diffs_json": []byte("[]"), "encrypted_payload": entries[index].EncryptedPayload,
			}).Error; err != nil {
				return err
			}
		}
		return nil
	})
}
