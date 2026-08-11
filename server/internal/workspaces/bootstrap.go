package workspaces

import (
	"context"
	"errors"

	"resolved-server/internal/identity"

	"github.com/google/uuid"
	"gorm.io/gorm"
	"gorm.io/gorm/clause"
)

const DefaultWorkspaceName = "My Workspace"

const initialWorkspaceBootstrapKey = "initial-workspace"

// SetupFirstOwnerWorkspace creates the initial workspace in the same
// transaction as the first owner, so bootstrap cannot leave a partially
// initialized deployment.
func SetupFirstOwnerWorkspace(ctx context.Context, tx *gorm.DB, owner identity.User) error {
	return createInitialWorkspace(ctx, tx, owner.ID)
}

// EnsureInitialWorkspace upgrades deployments bootstrapped before the initial
// workspace was introduced. Deployments without an owner remain untouched
// until bootstrap creates both records atomically.
func EnsureInitialWorkspace(ctx context.Context, db *gorm.DB) error {
	return db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var workspaceCount int64
		if err := tx.Model(&Workspace{}).Count(&workspaceCount).Error; err != nil {
			return err
		}
		if workspaceCount > 0 {
			return markInitialWorkspaceCreated(tx)
		}

		var marker identity.BootstrapState
		err := tx.First(&marker, "key = ?", initialWorkspaceBootstrapKey).Error
		switch {
		case err == nil:
			return nil
		case !errors.Is(err, gorm.ErrRecordNotFound):
			return err
		}

		var owner identity.User
		err = tx.Table("users").
			Joins("JOIN user_roles ON user_roles.user_id = users.id").
			Where("users.active = ? AND user_roles.role_id = ?", true, identity.OwnerRoleID).
			Order("users.created_at ASC").
			Order("users.id ASC").
			First(&owner).Error
		switch {
		case errors.Is(err, gorm.ErrRecordNotFound):
			return nil
		case err != nil:
			return err
		default:
			return createInitialWorkspace(ctx, tx, owner.ID)
		}
	})
}

func createInitialWorkspace(ctx context.Context, tx *gorm.DB, ownerID string) error {
	marker := identity.BootstrapState{Key: initialWorkspaceBootstrapKey}
	result := tx.WithContext(ctx).Clauses(clause.OnConflict{DoNothing: true}).Create(&marker)
	if result.Error != nil {
		return result.Error
	}
	if result.RowsAffected == 0 {
		return nil
	}

	workspace := Workspace{ID: uuid.NewString(), Name: DefaultWorkspaceName, CreatedByUserID: &ownerID}
	if err := tx.WithContext(ctx).Create(&workspace).Error; err != nil {
		return err
	}
	return tx.WithContext(ctx).Create(&WorkspaceUser{
		WorkspaceID: workspace.ID,
		UserID:      ownerID,
	}).Error
}

func markInitialWorkspaceCreated(tx *gorm.DB) error {
	return tx.Clauses(clause.OnConflict{DoNothing: true}).Create(
		&identity.BootstrapState{Key: initialWorkspaceBootstrapKey},
	).Error
}
