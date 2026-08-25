package workspaces

import (
	"context"
	"strings"
	"unicode/utf8"

	"resolved-server/internal/problem"
	"resolved-server/internal/resourceevents"
	"resolved-server/internal/security"
	"resolved-server/internal/validation"

	"github.com/google/uuid"
)

const maximumEnvironmentValueBytes = 1024 * 1024

type CreateEnvironmentInput struct {
	Name string
}

type UpdateEnvironmentInput struct {
	Name string
}

type CreateEnvironmentVariableInput struct {
	Key     string
	Value   string
	Enabled bool
	Secret  bool
}

type UpdateEnvironmentVariableInput struct {
	Key     *string
	Enabled *bool
	Secret  *bool
}

func (s *Service) ListEnvironments(
	ctx context.Context,
	actor Actor,
	workspaceID string,
) ([]Environment, error) {
	if err := validation.ID("workspace_id", workspaceID); err != nil {
		return nil, err
	}
	if _, err := s.environmentWorkspace(ctx, actor, workspaceID, false); err != nil {
		return nil, err
	}
	environments, err := s.environments.ListEnvironments(ctx, workspaceID)
	if err != nil {
		return nil, mapRepositoryError(err)
	}
	if err := s.hydrateEnvironmentValues(ctx, actor, workspaceID, environments); err != nil {
		return nil, err
	}
	return environments, nil
}

func (s *Service) GetEnvironment(
	ctx context.Context,
	actor Actor,
	workspaceID, environmentID string,
) (Environment, error) {
	if err := validateWorkspaceEnvironmentIDs(workspaceID, environmentID); err != nil {
		return Environment{}, err
	}
	if _, err := s.environmentWorkspace(ctx, actor, workspaceID, false); err != nil {
		return Environment{}, err
	}
	environment, err := s.environments.GetEnvironment(ctx, workspaceID, environmentID)
	if err != nil {
		return Environment{}, mapRepositoryError(err)
	}
	environments := []Environment{environment}
	if err := s.hydrateEnvironmentValues(ctx, actor, workspaceID, environments); err != nil {
		return Environment{}, err
	}
	return environments[0], nil
}

func (s *Service) CreateEnvironment(
	ctx context.Context,
	actor Actor,
	workspaceID string,
	input CreateEnvironmentInput,
) (Environment, error) {
	if err := validation.ID("workspace_id", workspaceID); err != nil {
		return Environment{}, err
	}
	workspace, err := s.environmentWorkspace(ctx, actor, workspaceID, true)
	if err != nil {
		return Environment{}, err
	}
	name, err := normalizeName(input.Name)
	if err != nil {
		return Environment{}, err
	}
	creatorID := actor.UserID
	environment, err := s.environments.CreateEnvironment(ctx, Environment{
		ID:              uuid.NewString(),
		WorkspaceID:     workspaceID,
		Name:            name,
		CreatedByUserID: &creatorID,
	})
	if err != nil {
		return Environment{}, mapRepositoryError(err)
	}
	s.publishChange(resourceevents.Change{
		Resource:      resourceevents.ResourceEnvironment,
		Action:        resourceevents.ActionCreated,
		ResourceID:    environment.ID,
		WorkspaceID:   workspaceID,
		EnvironmentID: environment.ID,
		Audience:      ownerScopedAudience(workspaceAudience(workspace)),
	})
	return environment, nil
}

func (s *Service) UpdateEnvironment(
	ctx context.Context,
	actor Actor,
	workspaceID, environmentID string,
	input UpdateEnvironmentInput,
) (Environment, error) {
	if err := validateWorkspaceEnvironmentIDs(workspaceID, environmentID); err != nil {
		return Environment{}, err
	}
	workspace, err := s.environmentWorkspace(ctx, actor, workspaceID, true)
	if err != nil {
		return Environment{}, err
	}
	name, err := normalizeName(input.Name)
	if err != nil {
		return Environment{}, err
	}
	environment, err := s.environments.UpdateEnvironmentName(ctx, workspaceID, environmentID, name)
	if err != nil {
		return Environment{}, mapRepositoryError(err)
	}
	environments := []Environment{environment}
	if err := s.hydrateEnvironmentValues(ctx, actor, workspaceID, environments); err != nil {
		return Environment{}, err
	}
	s.publishChange(resourceevents.Change{
		Resource:      resourceevents.ResourceEnvironment,
		Action:        resourceevents.ActionUpdated,
		ResourceID:    environment.ID,
		WorkspaceID:   workspaceID,
		EnvironmentID: environment.ID,
		Audience:      ownerScopedAudience(workspaceAudience(workspace)),
	})
	return environments[0], nil
}

func (s *Service) DeleteEnvironment(
	ctx context.Context,
	actor Actor,
	workspaceID, environmentID string,
) error {
	if err := validateWorkspaceEnvironmentIDs(workspaceID, environmentID); err != nil {
		return err
	}
	workspace, err := s.environmentWorkspace(ctx, actor, workspaceID, true)
	if err != nil {
		return err
	}
	if err := s.environments.DeleteEnvironment(ctx, workspaceID, environmentID); err != nil {
		return mapRepositoryError(err)
	}
	s.publishChange(resourceevents.Change{
		Resource:      resourceevents.ResourceEnvironment,
		Action:        resourceevents.ActionDeleted,
		ResourceID:    environmentID,
		WorkspaceID:   workspaceID,
		EnvironmentID: environmentID,
		Audience:      ownerScopedAudience(workspaceAudience(workspace)),
	})
	return nil
}

func (s *Service) CreateEnvironmentVariable(
	ctx context.Context,
	actor Actor,
	workspaceID, environmentID string,
	input CreateEnvironmentVariableInput,
) (EnvironmentVariable, error) {
	if err := validateWorkspaceEnvironmentIDs(workspaceID, environmentID); err != nil {
		return EnvironmentVariable{}, err
	}
	workspace, err := s.environmentWorkspace(ctx, actor, workspaceID, true)
	if err != nil {
		return EnvironmentVariable{}, err
	}
	key, err := normalizeEnvironmentVariableKey(input.Key)
	if err != nil {
		return EnvironmentVariable{}, err
	}
	if err := validateEnvironmentValue(input.Value); err != nil {
		return EnvironmentVariable{}, err
	}
	variableID := uuid.NewString()
	ciphertext, err := s.environmentCipher.EncryptValue(actor.EnvironmentKey, actor.UserID, variableID, input.Value)
	if err != nil {
		return EnvironmentVariable{}, problem.Wrap(err, "encrypt environment variable value")
	}
	creatorID := actor.UserID
	variable, err := s.environments.CreateEnvironmentVariable(
		ctx,
		workspaceID,
		EnvironmentVariable{
			ID:              variableID,
			EnvironmentID:   environmentID,
			Key:             key,
			Enabled:         input.Enabled,
			Secret:          input.Secret,
			CreatedByUserID: &creatorID,
		},
		actor.UserID,
		ciphertext,
	)
	if err != nil {
		return EnvironmentVariable{}, mapRepositoryError(err)
	}
	variable.Value = input.Value
	s.publishChange(resourceevents.Change{
		Resource:      resourceevents.ResourceEnvironmentVariable,
		Action:        resourceevents.ActionCreated,
		ResourceID:    variable.ID,
		WorkspaceID:   workspaceID,
		EnvironmentID: environmentID,
		Audience:      ownerScopedAudience(workspaceAudience(workspace)),
	})
	return variable, nil
}

func (s *Service) UpdateEnvironmentVariable(
	ctx context.Context,
	actor Actor,
	workspaceID, environmentID, variableID string,
	input UpdateEnvironmentVariableInput,
) (EnvironmentVariable, error) {
	if err := validateWorkspaceEnvironmentVariableIDs(workspaceID, environmentID, variableID); err != nil {
		return EnvironmentVariable{}, err
	}
	workspace, err := s.environmentWorkspace(ctx, actor, workspaceID, true)
	if err != nil {
		return EnvironmentVariable{}, err
	}
	if input.Key == nil && input.Enabled == nil && input.Secret == nil {
		return EnvironmentVariable{}, problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"body": "must contain at least one change"},
		)
	}
	var key *string
	if input.Key != nil {
		normalized, err := normalizeEnvironmentVariableKey(*input.Key)
		if err != nil {
			return EnvironmentVariable{}, err
		}
		key = &normalized
	}
	variable, err := s.environments.UpdateEnvironmentVariable(
		ctx, workspaceID, environmentID, variableID, key, input.Enabled, input.Secret,
	)
	if err != nil {
		return EnvironmentVariable{}, mapRepositoryError(err)
	}
	variable, err = s.hydrateEnvironmentVariableValue(ctx, actor, workspaceID, variable)
	if err != nil {
		return EnvironmentVariable{}, err
	}
	s.publishChange(resourceevents.Change{
		Resource:      resourceevents.ResourceEnvironmentVariable,
		Action:        resourceevents.ActionUpdated,
		ResourceID:    variable.ID,
		WorkspaceID:   workspaceID,
		EnvironmentID: environmentID,
		Audience:      ownerScopedAudience(workspaceAudience(workspace)),
	})
	return variable, nil
}

func (s *Service) PutEnvironmentVariableValue(
	ctx context.Context,
	actor Actor,
	workspaceID, environmentID, variableID, value string,
) (EnvironmentVariable, error) {
	if err := validateWorkspaceEnvironmentVariableIDs(workspaceID, environmentID, variableID); err != nil {
		return EnvironmentVariable{}, err
	}
	if _, err := s.environmentWorkspace(ctx, actor, workspaceID, false); err != nil {
		return EnvironmentVariable{}, err
	}
	if err := validateEnvironmentValue(value); err != nil {
		return EnvironmentVariable{}, err
	}
	ciphertext, err := s.environmentCipher.EncryptValue(actor.EnvironmentKey, actor.UserID, variableID, value)
	if err != nil {
		return EnvironmentVariable{}, problem.Wrap(err, "encrypt environment variable value")
	}
	variable, err := s.environments.PutEnvironmentVariableValue(
		ctx, workspaceID, environmentID, variableID, actor.UserID, ciphertext,
	)
	if err != nil {
		return EnvironmentVariable{}, mapRepositoryError(err)
	}
	variable.Value = value
	s.publishChange(resourceevents.Change{
		Resource:      resourceevents.ResourceEnvironmentVariable,
		Action:        resourceevents.ActionUpdated,
		ResourceID:    variable.ID,
		WorkspaceID:   workspaceID,
		EnvironmentID: environmentID,
		Audience:      resourceevents.Audience{UserIDs: []string{actor.UserID}},
	})
	return variable, nil
}

func (s *Service) DeleteEnvironmentVariable(
	ctx context.Context,
	actor Actor,
	workspaceID, environmentID, variableID string,
) error {
	if err := validateWorkspaceEnvironmentVariableIDs(workspaceID, environmentID, variableID); err != nil {
		return err
	}
	workspace, err := s.environmentWorkspace(ctx, actor, workspaceID, true)
	if err != nil {
		return err
	}
	if err := s.environments.DeleteEnvironmentVariable(ctx, workspaceID, environmentID, variableID); err != nil {
		return mapRepositoryError(err)
	}
	s.publishChange(resourceevents.Change{
		Resource:      resourceevents.ResourceEnvironmentVariable,
		Action:        resourceevents.ActionDeleted,
		ResourceID:    variableID,
		WorkspaceID:   workspaceID,
		EnvironmentID: environmentID,
		Audience:      ownerScopedAudience(workspaceAudience(workspace)),
	})
	return nil
}

func (s *Service) environmentWorkspace(
	ctx context.Context,
	actor Actor,
	workspaceID string,
	requireDirectGrant bool,
) (Workspace, error) {
	workspace, err := s.repository.GetWorkspace(ctx, workspaceID)
	if err != nil {
		return Workspace{}, mapRepositoryError(err)
	}
	if requireDirectGrant {
		if !hasWorkspaceGrant(workspace, actor) {
			return Workspace{}, workspaceAccessDenied()
		}
		return workspace, nil
	}
	if _, ok := scopeWorkspace(workspace, actor); !ok {
		return Workspace{}, workspaceAccessDenied()
	}
	return workspace, nil
}

func (s *Service) hydrateEnvironmentValues(
	ctx context.Context,
	actor Actor,
	workspaceID string,
	environments []Environment,
) error {
	if len(actor.EnvironmentKey) != security.EnvironmentKeyLength {
		return problem.Wrap(nil, "authenticated session is missing an environment key")
	}
	values, err := s.environments.ListEnvironmentValueCiphertexts(ctx, workspaceID, actor.UserID)
	if err != nil {
		return problem.Wrap(err, "load environment variable values")
	}
	for environmentIndex := range environments {
		for variableIndex := range environments[environmentIndex].Variables {
			variable := &environments[environmentIndex].Variables[variableIndex]
			ciphertext, exists := values[variable.ID]
			if !exists {
				variable.Value = ""
				continue
			}
			value, err := s.environmentCipher.DecryptValue(actor.EnvironmentKey, actor.UserID, variable.ID, ciphertext)
			if err != nil {
				return problem.Wrap(err, "decrypt environment variable value")
			}
			variable.Value = value
		}
	}
	return nil
}

func (s *Service) hydrateEnvironmentVariableValue(
	ctx context.Context,
	actor Actor,
	workspaceID string,
	variable EnvironmentVariable,
) (EnvironmentVariable, error) {
	environments := []Environment{{Variables: []EnvironmentVariable{variable}}}
	if err := s.hydrateEnvironmentValues(ctx, actor, workspaceID, environments); err != nil {
		return EnvironmentVariable{}, err
	}
	return environments[0].Variables[0], nil
}

func normalizeEnvironmentVariableKey(value string) (string, error) {
	key := strings.TrimSpace(value)
	if utf8.RuneCountInString(key) == 0 || utf8.RuneCountInString(key) > 256 ||
		strings.Contains(key, "{{") || strings.Contains(key, "}}") {
		return "", problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"key": "must contain 1 to 256 characters and cannot contain template braces"},
		)
	}
	return key, nil
}

func validateEnvironmentValue(value string) error {
	if len([]byte(value)) > maximumEnvironmentValueBytes {
		return problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"value": "must be no larger than 1 MiB"},
		)
	}
	return nil
}

func validateWorkspaceEnvironmentIDs(workspaceID, environmentID string) error {
	if err := validation.ID("workspace_id", workspaceID); err != nil {
		return err
	}
	return validation.ID("environment_id", environmentID)
}

func validateWorkspaceEnvironmentVariableIDs(workspaceID, environmentID, variableID string) error {
	if err := validateWorkspaceEnvironmentIDs(workspaceID, environmentID); err != nil {
		return err
	}
	return validation.ID("variable_id", variableID)
}
