package workspaces

import (
	"time"

	"resolved-server/internal/identity"
)

// Environment and EnvironmentVariable are workspace-wide definitions. They
// deliberately contain no persisted value field: values live in
// EnvironmentVariableValue and are scoped to one user.
type Environment struct {
	ID              string    `gorm:"type:char(36);primaryKey"`
	WorkspaceID     string    `gorm:"type:char(36);not null;index:environment_workspace_position,priority:1"`
	Workspace       Workspace `gorm:"foreignKey:WorkspaceID;references:ID;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	Name            string    `gorm:"size:120;not null"`
	EncryptedName   []byte
	Position        int                   `gorm:"not null;index:environment_workspace_position,priority:2"`
	Variables       []EnvironmentVariable `gorm:"-"`
	CreatedByUserID *string               `gorm:"type:char(36);index"`
	CreatedByUser   *identity.User        `gorm:"-"`
	CreatedAt       time.Time             `gorm:"not null"`
	UpdatedAt       time.Time             `gorm:"not null"`
}

type EnvironmentVariable struct {
	ID              string      `gorm:"type:char(36);primaryKey"`
	EnvironmentID   string      `gorm:"type:char(36);not null;uniqueIndex:environment_variable_key,priority:1;index:environment_variable_position,priority:1"`
	Environment     Environment `gorm:"foreignKey:EnvironmentID;references:ID;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	Key             string      `gorm:"size:256;not null"`
	KeyLookup       *string     `gorm:"size:64;uniqueIndex:environment_variable_key,priority:2"`
	EncryptedKey    []byte
	Position        int            `gorm:"not null;index:environment_variable_position,priority:2"`
	Enabled         bool           `gorm:"not null;default:true"`
	Secret          bool           `gorm:"not null;default:false"`
	Value           string         `gorm:"-"`
	CreatedByUserID *string        `gorm:"type:char(36);index"`
	CreatedByUser   *identity.User `gorm:"-"`
	CreatedAt       time.Time      `gorm:"not null"`
	UpdatedAt       time.Time      `gorm:"not null"`
}

type EnvironmentVariableValue struct {
	EnvironmentVariableID string              `gorm:"type:char(36);primaryKey"`
	EnvironmentVariable   EnvironmentVariable `gorm:"foreignKey:EnvironmentVariableID;references:ID;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	UserID                string              `gorm:"type:char(36);primaryKey;index"`
	User                  identity.User       `gorm:"foreignKey:UserID;references:ID;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	Ciphertext            []byte              `gorm:"not null"`
	CreatedAt             time.Time           `gorm:"not null"`
	UpdatedAt             time.Time           `gorm:"not null"`
}

type EnvironmentView struct {
	ID          string                    `json:"id"`
	WorkspaceID string                    `json:"workspace_id"`
	Name        string                    `json:"name"`
	Variables   []EnvironmentVariableView `json:"variables"`
	CreatedBy   *identity.UserSummaryView `json:"created_by"`
	CreatedAt   time.Time                 `json:"created_at"`
	UpdatedAt   time.Time                 `json:"updated_at"`
}

type EnvironmentVariableView struct {
	ID            string                    `json:"id"`
	EnvironmentID string                    `json:"environment_id"`
	Key           string                    `json:"key"`
	Value         string                    `json:"value"`
	Enabled       bool                      `json:"enabled"`
	Secret        bool                      `json:"secret"`
	CreatedBy     *identity.UserSummaryView `json:"created_by"`
	CreatedAt     time.Time                 `json:"created_at"`
	UpdatedAt     time.Time                 `json:"updated_at"`
}

func ViewEnvironment(environment Environment) EnvironmentView {
	return EnvironmentView{
		ID:          environment.ID,
		WorkspaceID: environment.WorkspaceID,
		Name:        environment.Name,
		Variables:   ViewEnvironmentVariables(environment.Variables),
		CreatedBy:   identity.ViewUserSummary(environment.CreatedByUser),
		CreatedAt:   environment.CreatedAt,
		UpdatedAt:   environment.UpdatedAt,
	}
}

func ViewEnvironments(environments []Environment) []EnvironmentView {
	views := make([]EnvironmentView, 0, len(environments))
	for _, environment := range environments {
		views = append(views, ViewEnvironment(environment))
	}
	return views
}

func ViewEnvironmentVariable(variable EnvironmentVariable) EnvironmentVariableView {
	return EnvironmentVariableView{
		ID:            variable.ID,
		EnvironmentID: variable.EnvironmentID,
		Key:           variable.Key,
		Value:         variable.Value,
		Enabled:       variable.Enabled,
		Secret:        variable.Secret,
		CreatedBy:     identity.ViewUserSummary(variable.CreatedByUser),
		CreatedAt:     variable.CreatedAt,
		UpdatedAt:     variable.UpdatedAt,
	}
}

func ViewEnvironmentVariables(variables []EnvironmentVariable) []EnvironmentVariableView {
	views := make([]EnvironmentVariableView, 0, len(variables))
	for _, variable := range variables {
		views = append(views, ViewEnvironmentVariable(variable))
	}
	return views
}
