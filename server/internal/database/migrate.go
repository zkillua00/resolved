package database

import (
	"context"
	"errors"
	"fmt"

	"resolved-server/internal/activitylog"
	"resolved-server/internal/config"
	"resolved-server/internal/identity"
	"resolved-server/internal/requestproxy"
	"resolved-server/internal/security"
	"resolved-server/internal/sharedhistory"
	"resolved-server/internal/workspaces"

	"gorm.io/gorm"
	"gorm.io/gorm/clause"
)

const DataEncryptionMigrationState = "server-data-encryption-v1"

func MigrateAndSeed(db *gorm.DB) error {
	if !db.Migrator().HasColumn(&sharedhistory.Entry{}, "ClientEntryLookup") &&
		db.Migrator().HasIndex(&sharedhistory.Entry{}, "idx_history_origin") {
		if err := db.Migrator().DropIndex(&sharedhistory.Entry{}, "idx_history_origin"); err != nil {
			return fmt.Errorf("remove plaintext shared-history origin index: %w", err)
		}
	}
	if !db.Migrator().HasColumn(&workspaces.EnvironmentVariable{}, "KeyLookup") &&
		db.Migrator().HasIndex(&workspaces.EnvironmentVariable{}, "environment_variable_key") {
		if err := db.Migrator().DropIndex(&workspaces.EnvironmentVariable{}, "environment_variable_key"); err != nil {
			return fmt.Errorf("remove plaintext environment-variable key index: %w", err)
		}
	}
	if err := db.AutoMigrate(
		&identity.Permission{},
		&identity.Role{},
		&identity.User{},
		&identity.Session{},
		&identity.BootstrapState{},
		&security.DataEncryptionKey{},
		&requestproxy.SettingsRecord{},
		&requestproxy.HostnameOverrideRecord{},
		&workspaces.Workspace{},
		&workspaces.Collection{},
		&workspaces.SavedRequest{},
		&workspaces.WorkspaceUser{},
		&workspaces.CollectionUser{},
		&workspaces.Environment{},
		&workspaces.EnvironmentVariable{},
		&workspaces.EnvironmentVariableValue{},
		&sharedhistory.Entry{},
		&activitylog.Entry{},
	); err != nil {
		return fmt.Errorf("migrate server schema: %w", err)
	}
	if db.Migrator().HasIndex(&identity.User{}, "idx_users_email") {
		if err := db.Migrator().DropIndex(&identity.User{}, "idx_users_email"); err != nil {
			return fmt.Errorf("remove plaintext user email index: %w", err)
		}
	}
	if db.Migrator().HasIndex(&identity.Role{}, "idx_roles_normalized_name") {
		if err := db.Migrator().DropIndex(&identity.Role{}, "idx_roles_normalized_name"); err != nil {
			return fmt.Errorf("remove plaintext role-name index: %w", err)
		}
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

		settings := requestproxy.SettingsRecord{
			ID:   requestproxy.SettingsRecordID,
			Mode: requestproxy.ModeLocal,
		}
		if err := tx.Clauses(clause.OnConflict{DoNothing: true}).Create(&settings).Error; err != nil {
			return fmt.Errorf("seed request execution settings: %w", err)
		}
		return nil
	})
}

func FinalizeDataEncryptionMigration(ctx context.Context, db *gorm.DB, cfg config.Database) error {
	if cfg.Driver != "sqlite" || sqlitePath(cfg.DSN) == "" {
		return nil
	}
	var marker identity.BootstrapState
	err := db.WithContext(ctx).First(&marker, "key = ?", DataEncryptionMigrationState).Error
	if err == nil {
		return nil
	}
	if !errors.Is(err, gorm.ErrRecordNotFound) {
		return fmt.Errorf("load data encryption migration state: %w", err)
	}
	if err := db.WithContext(ctx).Exec("PRAGMA wal_checkpoint(TRUNCATE)").Error; err != nil {
		return fmt.Errorf("truncate SQLite WAL after data encryption migration: %w", err)
	}
	if err := db.WithContext(ctx).Exec("VACUUM").Error; err != nil {
		return fmt.Errorf("rebuild SQLite after data encryption migration: %w", err)
	}
	marker = identity.BootstrapState{Key: DataEncryptionMigrationState}
	if err := db.WithContext(ctx).Create(&marker).Error; err != nil {
		return fmt.Errorf("record data encryption migration state: %w", err)
	}
	return nil
}
