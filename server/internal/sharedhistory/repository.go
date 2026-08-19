package sharedhistory

import (
	"context"

	"resolved-server/internal/identity"

	"gorm.io/gorm"
	"gorm.io/gorm/clause"
)

type Repository struct {
	db *gorm.DB
}

func NewRepository(db *gorm.DB) *Repository {
	return &Repository{db: db}
}

func (r *Repository) ListProfiles(ctx context.Context) ([]identity.User, error) {
	var users []identity.User
	err := r.db.WithContext(ctx).
		Order("display_name ASC, email ASC, id ASC").
		Find(&users).Error
	return users, err
}

func (r *Repository) GetProfile(ctx context.Context, userID string) (identity.User, error) {
	var user identity.User
	err := r.db.WithContext(ctx).First(&user, "id = ?", userID).Error
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
		if err := tx.Clauses(clause.OnConflict{
			Columns: []clause.Column{
				{Name: "workspace_id"},
				{Name: "user_id"},
				{Name: "client_entry_id"},
			},
			DoUpdates: clause.AssignmentColumns([]string{
				"method", "url", "request_headers_json", "request_body",
				"request_body_mode", "request_body_language", "request_body_fields_json",
				"request_body_truncated", "response_status", "response_status_text",
				"response_http_version", "response_final_url", "response_headers_json",
				"response_body", "response_body_truncated", "response_content_type",
				"response_duration_micros", "error", "created_at", "updated_at",
			}),
		}).Create(&entry).Error; err != nil {
			return err
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
		if err := tx.First(
			&stored,
			"workspace_id = ? AND user_id = ? AND client_entry_id = ?",
			entry.WorkspaceID,
			entry.UserID,
			entry.ClientEntryID,
		).Error; err != nil {
			return err
		}
		entry = stored
		return nil
	})
	return entry, err
}

func (r *Repository) ListEntries(ctx context.Context, workspaceID, userID string) ([]Entry, error) {
	var entries []Entry
	err := r.db.WithContext(ctx).
		Where("workspace_id = ? AND user_id = ?", workspaceID, userID).
		Order("created_at DESC, id DESC").
		Limit(MaxListedEntries).
		Find(&entries).Error
	return entries, err
}

func (r *Repository) DeleteEntries(ctx context.Context, workspaceID, userID string) error {
	return r.db.WithContext(ctx).
		Where("workspace_id = ? AND user_id = ?", workspaceID, userID).
		Delete(&Entry{}).Error
}
