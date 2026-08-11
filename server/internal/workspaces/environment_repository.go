package workspaces

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"time"

	"resolved-server/internal/security"

	"gorm.io/gorm"
	"gorm.io/gorm/clause"
)

func (r *Repository) ListEnvironments(ctx context.Context, workspaceID string) ([]Environment, error) {
	db := r.db.WithContext(ctx)
	if err := ensureWorkspaceExists(db, workspaceID); err != nil {
		return nil, err
	}
	return loadEnvironments(db, workspaceID)
}

func (r *Repository) GetEnvironment(ctx context.Context, workspaceID, environmentID string) (Environment, error) {
	var environment Environment
	db := r.db.WithContext(ctx)
	if err := db.Where("workspace_id = ? AND id = ?", workspaceID, environmentID).
		First(&environment).Error; err != nil {
		if errors.Is(err, gorm.ErrRecordNotFound) {
			return Environment{}, ErrEnvironmentNotFound
		}
		return Environment{}, err
	}
	if err := hydrateCreator(db, environment.CreatedByUserID, &environment.CreatedByUser); err != nil {
		return Environment{}, err
	}
	variables, err := loadEnvironmentVariables(db, []string{environment.ID})
	if err != nil {
		return Environment{}, err
	}
	environment.Variables = variables[environment.ID]
	if environment.Variables == nil {
		environment.Variables = []EnvironmentVariable{}
	}
	return environment, nil
}

func (r *Repository) CreateEnvironment(ctx context.Context, environment Environment) (Environment, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureWorkspaceExists(tx, environment.WorkspaceID); err != nil {
			return err
		}
		position, err := nextEnvironmentPosition(tx, environment.WorkspaceID)
		if err != nil {
			return err
		}
		environment.Position = position
		if err := tx.Create(&environment).Error; err != nil {
			return err
		}
		return touchWorkspace(tx, environment.WorkspaceID)
	})
	if err != nil {
		return Environment{}, err
	}
	return r.GetEnvironment(ctx, environment.WorkspaceID, environment.ID)
}

func (r *Repository) UpdateEnvironmentName(
	ctx context.Context,
	workspaceID, environmentID, name string,
) (Environment, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureEnvironmentExists(tx, workspaceID, environmentID); err != nil {
			return err
		}
		result := tx.Model(&Environment{}).
			Where("workspace_id = ? AND id = ?", workspaceID, environmentID).
			Update("name", name)
		if result.Error != nil {
			return result.Error
		}
		return touchWorkspace(tx, workspaceID)
	})
	if err != nil {
		return Environment{}, err
	}
	return r.GetEnvironment(ctx, workspaceID, environmentID)
}

func (r *Repository) DeleteEnvironment(ctx context.Context, workspaceID, environmentID string) error {
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureEnvironmentExists(tx, workspaceID, environmentID); err != nil {
			return err
		}
		variableIDs := tx.Model(&EnvironmentVariable{}).Select("id").Where("environment_id = ?", environmentID)
		if err := tx.Where("environment_variable_id IN (?)", variableIDs).
			Delete(&EnvironmentVariableValue{}).Error; err != nil {
			return err
		}
		if err := tx.Where("environment_id = ?", environmentID).Delete(&EnvironmentVariable{}).Error; err != nil {
			return err
		}
		if err := tx.Where("workspace_id = ? AND id = ?", workspaceID, environmentID).
			Delete(&Environment{}).Error; err != nil {
			return err
		}
		return touchWorkspace(tx, workspaceID)
	})
}

func (r *Repository) CreateEnvironmentVariable(
	ctx context.Context,
	workspaceID string,
	variable EnvironmentVariable,
	userID string,
	ciphertext []byte,
) (EnvironmentVariable, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureEnvironmentExists(tx, workspaceID, variable.EnvironmentID); err != nil {
			return err
		}
		position, err := nextEnvironmentVariablePosition(tx, variable.EnvironmentID)
		if err != nil {
			return err
		}
		variable.Position = position
		if err := tx.Create(&variable).Error; err != nil {
			if errors.Is(err, gorm.ErrDuplicatedKey) {
				return ErrEnvironmentVariableKeyExists
			}
			return err
		}
		value := EnvironmentVariableValue{
			EnvironmentVariableID: variable.ID,
			UserID:                userID,
			Ciphertext:            ciphertext,
		}
		if err := tx.Create(&value).Error; err != nil {
			return err
		}
		if err := touchEnvironment(tx, variable.EnvironmentID); err != nil {
			return err
		}
		return touchWorkspace(tx, workspaceID)
	})
	if err != nil {
		return EnvironmentVariable{}, err
	}
	return r.GetEnvironmentVariable(ctx, workspaceID, variable.EnvironmentID, variable.ID)
}

func (r *Repository) GetEnvironmentVariable(
	ctx context.Context,
	workspaceID, environmentID, variableID string,
) (EnvironmentVariable, error) {
	var variable EnvironmentVariable
	db := r.db.WithContext(ctx)
	err := db.Model(&EnvironmentVariable{}).
		Joins("JOIN environments ON environments.id = environment_variables.environment_id").
		Where(
			"environments.workspace_id = ? AND environment_variables.environment_id = ? AND environment_variables.id = ?",
			workspaceID,
			environmentID,
			variableID,
		).
		Select("environment_variables.*").
		First(&variable).Error
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return EnvironmentVariable{}, ErrEnvironmentVariableNotFound
	}
	if err != nil {
		return EnvironmentVariable{}, err
	}
	if err := hydrateCreator(db, variable.CreatedByUserID, &variable.CreatedByUser); err != nil {
		return EnvironmentVariable{}, err
	}
	return variable, nil
}

func (r *Repository) UpdateEnvironmentVariable(
	ctx context.Context,
	workspaceID, environmentID, variableID string,
	key *string,
	enabled, secret *bool,
) (EnvironmentVariable, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureEnvironmentVariableExists(tx, workspaceID, environmentID, variableID); err != nil {
			return err
		}
		updates := map[string]any{}
		if key != nil {
			updates["key"] = *key
		}
		if enabled != nil {
			updates["enabled"] = *enabled
		}
		if secret != nil {
			updates["secret"] = *secret
		}
		result := tx.Model(&EnvironmentVariable{}).
			Where("environment_id = ? AND id = ?", environmentID, variableID).
			Updates(updates)
		if result.Error != nil {
			if errors.Is(result.Error, gorm.ErrDuplicatedKey) {
				return ErrEnvironmentVariableKeyExists
			}
			return result.Error
		}
		if err := touchEnvironment(tx, environmentID); err != nil {
			return err
		}
		return touchWorkspace(tx, workspaceID)
	})
	if err != nil {
		return EnvironmentVariable{}, err
	}
	return r.GetEnvironmentVariable(ctx, workspaceID, environmentID, variableID)
}

func (r *Repository) PutEnvironmentVariableValue(
	ctx context.Context,
	workspaceID, environmentID, variableID, userID string,
	ciphertext []byte,
) (EnvironmentVariable, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureEnvironmentVariableExists(tx, workspaceID, environmentID, variableID); err != nil {
			return err
		}
		value := EnvironmentVariableValue{
			EnvironmentVariableID: variableID,
			UserID:                userID,
			Ciphertext:            ciphertext,
		}
		return tx.Clauses(clause.OnConflict{
			Columns: []clause.Column{{Name: "environment_variable_id"}, {Name: "user_id"}},
			DoUpdates: clause.Assignments(map[string]any{
				"ciphertext": ciphertext,
				"updated_at": time.Now().UTC(),
			}),
		}).Create(&value).Error
	})
	if err != nil {
		return EnvironmentVariable{}, err
	}
	return r.GetEnvironmentVariable(ctx, workspaceID, environmentID, variableID)
}

func (r *Repository) DeleteEnvironmentVariable(
	ctx context.Context,
	workspaceID, environmentID, variableID string,
) error {
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := ensureEnvironmentVariableExists(tx, workspaceID, environmentID, variableID); err != nil {
			return err
		}
		if err := tx.Where("environment_variable_id = ?", variableID).
			Delete(&EnvironmentVariableValue{}).Error; err != nil {
			return err
		}
		if err := tx.Where("environment_id = ? AND id = ?", environmentID, variableID).
			Delete(&EnvironmentVariable{}).Error; err != nil {
			return err
		}
		if err := touchEnvironment(tx, environmentID); err != nil {
			return err
		}
		return touchWorkspace(tx, workspaceID)
	})
}

func (r *Repository) ListEnvironmentValueCiphertexts(
	ctx context.Context,
	workspaceID, userID string,
) (map[string][]byte, error) {
	var values []EnvironmentVariableValue
	err := r.db.WithContext(ctx).Model(&EnvironmentVariableValue{}).
		Select("environment_variable_values.*").
		Joins("JOIN environment_variables ON environment_variables.id = environment_variable_values.environment_variable_id").
		Joins("JOIN environments ON environments.id = environment_variables.environment_id").
		Where("environments.workspace_id = ? AND environment_variable_values.user_id = ?", workspaceID, userID).
		Find(&values).Error
	if err != nil {
		return nil, err
	}
	result := make(map[string][]byte, len(values))
	for _, value := range values {
		result[value.EnvironmentVariableID] = append([]byte(nil), value.Ciphertext...)
	}
	return result, nil
}

func RekeyEnvironmentVariableValues(
	ctx context.Context,
	tx *gorm.DB,
	cipher *security.EnvironmentCipher,
	userID string,
	oldKey, newKey []byte,
) error {
	var values []EnvironmentVariableValue
	if err := tx.WithContext(ctx).Where("user_id = ?", userID).Find(&values).Error; err != nil {
		return err
	}
	if len(values) > 0 && len(oldKey) != security.EnvironmentKeyLength {
		return security.ErrEnvironmentKeyUnavailable
	}
	for _, value := range values {
		plaintext, err := cipher.DecryptValue(oldKey, userID, value.EnvironmentVariableID, value.Ciphertext)
		if err != nil {
			return fmt.Errorf("decrypt variable %s: %w", value.EnvironmentVariableID, err)
		}
		ciphertext, err := cipher.EncryptValue(newKey, userID, value.EnvironmentVariableID, plaintext)
		if err != nil {
			return fmt.Errorf("encrypt variable %s: %w", value.EnvironmentVariableID, err)
		}
		if err := tx.Model(&EnvironmentVariableValue{}).
			Where("environment_variable_id = ? AND user_id = ?", value.EnvironmentVariableID, userID).
			Updates(map[string]any{
				"ciphertext": ciphertext,
				"updated_at": time.Now().UTC(),
			}).Error; err != nil {
			return err
		}
	}
	return nil
}

func loadEnvironments(db *gorm.DB, workspaceID string) ([]Environment, error) {
	var environments []Environment
	if err := db.Where("workspace_id = ?", workspaceID).
		Order("position ASC").Order("id ASC").Find(&environments).Error; err != nil {
		return nil, err
	}
	ids := make([]string, 0, len(environments))
	for index := range environments {
		ids = append(ids, environments[index].ID)
		if err := hydrateCreator(db, environments[index].CreatedByUserID, &environments[index].CreatedByUser); err != nil {
			return nil, err
		}
	}
	variables, err := loadEnvironmentVariables(db, ids)
	if err != nil {
		return nil, err
	}
	for index := range environments {
		environments[index].Variables = variables[environments[index].ID]
		if environments[index].Variables == nil {
			environments[index].Variables = []EnvironmentVariable{}
		}
	}
	return environments, nil
}

func loadEnvironmentVariables(db *gorm.DB, environmentIDs []string) (map[string][]EnvironmentVariable, error) {
	result := make(map[string][]EnvironmentVariable, len(environmentIDs))
	if len(environmentIDs) == 0 {
		return result, nil
	}
	var variables []EnvironmentVariable
	if err := db.Where("environment_id IN ?", environmentIDs).
		Order("environment_id ASC").Order("position ASC").Order("id ASC").
		Find(&variables).Error; err != nil {
		return nil, err
	}
	for index := range variables {
		if err := hydrateCreator(db, variables[index].CreatedByUserID, &variables[index].CreatedByUser); err != nil {
			return nil, err
		}
		result[variables[index].EnvironmentID] = append(result[variables[index].EnvironmentID], variables[index])
	}
	return result, nil
}

func ensureEnvironmentExists(tx *gorm.DB, workspaceID, environmentID string) error {
	var count int64
	if err := tx.Model(&Environment{}).
		Where("workspace_id = ? AND id = ?", workspaceID, environmentID).
		Count(&count).Error; err != nil {
		return err
	}
	if count == 0 {
		return ErrEnvironmentNotFound
	}
	return nil
}

func ensureEnvironmentVariableExists(
	tx *gorm.DB,
	workspaceID, environmentID, variableID string,
) error {
	var count int64
	if err := tx.Model(&EnvironmentVariable{}).
		Joins("JOIN environments ON environments.id = environment_variables.environment_id").
		Where(
			"environments.workspace_id = ? AND environment_variables.environment_id = ? AND environment_variables.id = ?",
			workspaceID,
			environmentID,
			variableID,
		).
		Count(&count).Error; err != nil {
		return err
	}
	if count == 0 {
		return ErrEnvironmentVariableNotFound
	}
	return nil
}

func touchEnvironment(tx *gorm.DB, environmentID string) error {
	return tx.Model(&Environment{}).Where("id = ?", environmentID).
		UpdateColumn("updated_at", time.Now().UTC()).Error
}

func nextEnvironmentPosition(tx *gorm.DB, workspaceID string) (int, error) {
	return nextPosition(tx.Model(&Environment{}).Where("workspace_id = ?", workspaceID))
}

func nextEnvironmentVariablePosition(tx *gorm.DB, environmentID string) (int, error) {
	return nextPosition(tx.Model(&EnvironmentVariable{}).Where("environment_id = ?", environmentID))
}

func nextPosition(query *gorm.DB) (int, error) {
	var maximum sql.NullInt64
	if err := query.Select("MAX(position)").Scan(&maximum).Error; err != nil {
		return 0, err
	}
	if !maximum.Valid {
		return 0, nil
	}
	return int(maximum.Int64 + 1), nil
}
