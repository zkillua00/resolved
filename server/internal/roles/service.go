package roles

import (
	"context"
	"errors"
	"strings"
	"unicode/utf8"

	"resolved-server/internal/identity"
	"resolved-server/internal/problem"

	"github.com/google/uuid"
)

type Service struct {
	repository *identity.Repository
}

type CreateInput struct {
	Name            string
	Description     string
	PermissionKeys  []string
	CreatedByUserID *string
}

type UpdateInput struct {
	Name        *string
	Description *string
}

func NewService(repository *identity.Repository) *Service {
	return &Service{repository: repository}
}

func (s *Service) List(ctx context.Context) ([]identity.Role, error) {
	roles, err := s.repository.ListRoles(ctx)
	if err != nil {
		return nil, problem.Wrap(err, "list roles")
	}
	return roles, nil
}

func (s *Service) Get(ctx context.Context, id string) (identity.Role, error) {
	if err := validateRoleID(id); err != nil {
		return identity.Role{}, err
	}
	role, err := s.repository.GetRole(ctx, id)
	if err != nil {
		return identity.Role{}, mapRepositoryError(err)
	}
	return role, nil
}

func (s *Service) Create(ctx context.Context, input CreateInput) (identity.Role, error) {
	name, normalizedName, err := normalizeName(input.Name)
	if err != nil {
		return identity.Role{}, err
	}
	role := identity.Role{
		ID:              uuid.NewString(),
		Name:            name,
		NormalizedName:  normalizedName,
		Description:     strings.TrimSpace(input.Description),
		System:          false,
		CreatedByUserID: input.CreatedByUserID,
	}
	created, err := s.repository.CreateRole(ctx, role, normalizePermissionKeys(input.PermissionKeys))
	if err != nil {
		return identity.Role{}, mapRepositoryError(err)
	}
	return created, nil
}

func (s *Service) Update(ctx context.Context, id string, input UpdateInput) (identity.Role, error) {
	if err := validateRoleID(id); err != nil {
		return identity.Role{}, err
	}
	changes := identity.RoleChanges{}
	if input.Name != nil {
		name, normalizedName, err := normalizeName(*input.Name)
		if err != nil {
			return identity.Role{}, err
		}
		changes.Name = &name
		changes.NormalizedName = &normalizedName
	}
	if input.Description != nil {
		description := strings.TrimSpace(*input.Description)
		changes.Description = &description
	}
	role, err := s.repository.UpdateRole(ctx, id, changes)
	if err != nil {
		return identity.Role{}, mapRepositoryError(err)
	}
	return role, nil
}

func (s *Service) ReplacePermissions(ctx context.Context, id string, keys []string) (identity.Role, error) {
	if err := validateRoleID(id); err != nil {
		return identity.Role{}, err
	}
	role, err := s.repository.ReplaceRolePermissions(ctx, id, normalizePermissionKeys(keys))
	if err != nil {
		return identity.Role{}, mapRepositoryError(err)
	}
	return role, nil
}

func (s *Service) ListPermissions(ctx context.Context) ([]identity.Permission, error) {
	permissions, err := s.repository.ListPermissions(ctx)
	if err != nil {
		return nil, problem.Wrap(err, "list permissions")
	}
	return permissions, nil
}

func normalizeName(value string) (string, string, error) {
	name := strings.TrimSpace(value)
	length := utf8.RuneCountInString(name)
	if length < 2 || length > 100 {
		return "", "", problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"name": "must contain between 2 and 100 characters"},
		)
	}
	return name, strings.ToLower(name), nil
}

func validateRoleID(id string) error {
	if _, err := uuid.Parse(id); err != nil {
		return problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"id": "must be a valid UUID"},
		)
	}
	return nil
}

func normalizePermissionKeys(keys []string) []string {
	normalized := make([]string, 0, len(keys))
	for _, key := range keys {
		normalized = append(normalized, strings.ToLower(strings.TrimSpace(key)))
	}
	return normalized
}

func mapRepositoryError(err error) error {
	switch {
	case errors.Is(err, identity.ErrRoleNotFound):
		return problem.New(problem.KindNotFound, "role_not_found", "role was not found")
	case errors.Is(err, identity.ErrRoleNameExists):
		return problem.New(problem.KindConflict, "role_name_exists", "a role with this name already exists")
	case errors.Is(err, identity.ErrUnknownPermission):
		return problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"permission_keys": "contains an unknown permission"},
		)
	case errors.Is(err, identity.ErrSystemRoleImmutable):
		return problem.New(problem.KindForbidden, "system_role_immutable", "system roles cannot be changed")
	default:
		return problem.Wrap(err, "persist role")
	}
}
