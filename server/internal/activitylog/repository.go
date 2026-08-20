package activitylog

import (
	"context"
	"errors"

	"resolved-server/internal/identity"

	"gorm.io/gorm"
)

type Repository struct {
	db *gorm.DB
}

func NewRepository(db *gorm.DB) *Repository {
	return &Repository{db: db}
}

func (r *Repository) Create(ctx context.Context, entry Entry) error {
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if entry.ActorUserID != "" {
			var actor identity.User
			if err := tx.Select("email", "display_name").First(&actor, "id = ?", entry.ActorUserID).Error; err == nil {
				entry.ActorEmail = actor.Email
				entry.ActorDisplayName = actor.DisplayName
			} else if err != nil && !errors.Is(err, gorm.ErrRecordNotFound) {
				return err
			}
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
	return listPage(query, page)
}

func (r *Repository) ListAudit(ctx context.Context, page pageQuery) (entryPage, error) {
	query := r.db.WithContext(ctx).Where("kind = ?", KindAudit)
	return listPage(query, page)
}

func listPage(query *gorm.DB, page pageQuery) (entryPage, error) {
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
	return entryPage{Entries: entries, HasMore: hasMore}, nil
}
