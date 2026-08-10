package database

import (
	"fmt"

	"resolved-server/internal/identity"
	"resolved-server/internal/workspaces"

	"gorm.io/gorm"
	"gorm.io/gorm/clause"
)

func MigrateAndSeed(db *gorm.DB) error {
	if err := db.AutoMigrate(
		&identity.Permission{},
		&identity.Role{},
		&identity.User{},
		&identity.Session{},
		&identity.BootstrapState{},
		&workspaces.Workspace{},
		&workspaces.Collection{},
		&workspaces.WorkspaceUser{},
		&workspaces.CollectionUser{},
	); err != nil {
		return fmt.Errorf("migrate server schema: %w", err)
	}

	return db.Transaction(func(tx *gorm.DB) error {
		permissions := identity.PermissionCatalog()
		for _, permission := range permissions {
			if err := tx.Clauses(clause.OnConflict{
				Columns:   []clause.Column{{Name: "key"}},
				DoUpdates: clause.AssignmentColumns([]string{"description"}),
			}).Create(&permission).Error; err != nil {
				return fmt.Errorf("seed permission %s: %w", permission.Key, err)
			}
		}

		owner := identity.Role{
			ID:             identity.OwnerRoleID,
			Name:           "Owner",
			NormalizedName: "owner",
			Description:    "Built-in deployment owner with every permission",
			System:         true,
		}
		if err := tx.Clauses(clause.OnConflict{
			Columns: []clause.Column{{Name: "id"}},
			DoUpdates: clause.Assignments(map[string]any{
				"name":            owner.Name,
				"normalized_name": owner.NormalizedName,
				"description":     owner.Description,
				"system":          true,
			}),
		}).Create(&owner).Error; err != nil {
			return fmt.Errorf("seed owner role: %w", err)
		}

		if err := tx.Model(&owner).Association("Permissions").Replace(&permissions); err != nil {
			return fmt.Errorf("reconcile owner permissions: %w", err)
		}
		return nil
	})
}
