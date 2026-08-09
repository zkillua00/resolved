package users

import (
	"context"
	"errors"
	"strings"

	"resolved-server/internal/identity"
	"resolved-server/internal/problem"
	"resolved-server/internal/security"

	"github.com/google/uuid"
)

type Service struct {
	repository *identity.Repository
	hasher     *security.PasswordHasher
}

type CreateInput struct {
	Email       string
	DisplayName string
	Password    string
	RoleIDs     []string
}

type UpdateInput struct {
	Email       *string
	DisplayName *string
	Password    *string
	Active      *bool
}

func NewService(repository *identity.Repository, hasher *security.PasswordHasher) *Service {
	return &Service{repository: repository, hasher: hasher}
}

func (s *Service) BootstrapOwner(ctx context.Context, input CreateInput) (identity.User, error) {
	user, err := s.newUser(input)
	if err != nil {
		return identity.User{}, err
	}
	created, err := s.repository.CreateFirstOwner(ctx, user)
	if err != nil {
		return identity.User{}, mapRepositoryError(err)
	}
	return created, nil
}

func (s *Service) Create(ctx context.Context, input CreateInput) (identity.User, error) {
	if err := validateIDs("role_ids", input.RoleIDs); err != nil {
		return identity.User{}, err
	}
	user, err := s.newUser(input)
	if err != nil {
		return identity.User{}, err
	}
	created, err := s.repository.CreateUser(ctx, user, input.RoleIDs)
	if err != nil {
		return identity.User{}, mapRepositoryError(err)
	}
	return created, nil
}

func (s *Service) List(ctx context.Context) ([]identity.User, error) {
	users, err := s.repository.ListUsers(ctx)
	if err != nil {
		return nil, problem.Wrap(err, "list users")
	}
	return users, nil
}

func (s *Service) Get(ctx context.Context, id string) (identity.User, error) {
	if err := validateID("id", id); err != nil {
		return identity.User{}, err
	}
	user, err := s.repository.GetUser(ctx, id)
	if err != nil {
		return identity.User{}, mapRepositoryError(err)
	}
	return user, nil
}

func (s *Service) Update(ctx context.Context, id string, input UpdateInput) (identity.User, error) {
	if err := validateID("id", id); err != nil {
		return identity.User{}, err
	}
	changes := identity.UserChanges{Active: input.Active}
	if input.Email != nil {
		login := normalizeLogin(*input.Email)
		if login == "" {
			return identity.User{}, problem.WithFields(
				"validation_failed",
				"request validation failed",
				map[string]string{"email": "is required"},
			)
		}
		changes.Email = &login
	}
	if input.DisplayName != nil {
		displayName := strings.TrimSpace(*input.DisplayName)
		if displayName == "" {
			return identity.User{}, problem.WithFields(
				"validation_failed",
				"request validation failed",
				map[string]string{"display_name": "cannot be empty"},
			)
		}
		changes.DisplayName = &displayName
	}
	if input.Password != nil {
		passwordHash, err := s.hashPassword(*input.Password)
		if err != nil {
			return identity.User{}, err
		}
		changes.PasswordHash = &passwordHash
	}

	user, err := s.repository.UpdateUser(ctx, id, changes)
	if err != nil {
		return identity.User{}, mapRepositoryError(err)
	}
	return user, nil
}

func (s *Service) ReplaceRoles(ctx context.Context, id string, roleIDs []string) (identity.User, error) {
	if err := validateID("id", id); err != nil {
		return identity.User{}, err
	}
	if err := validateIDs("role_ids", roleIDs); err != nil {
		return identity.User{}, err
	}
	user, err := s.repository.ReplaceUserRoles(ctx, id, roleIDs)
	if err != nil {
		return identity.User{}, mapRepositoryError(err)
	}
	return user, nil
}

func (s *Service) newUser(input CreateInput) (identity.User, error) {
	login := normalizeLogin(input.Email)
	if login == "" {
		return identity.User{}, problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"email": "is required"},
		)
	}
	displayName := strings.TrimSpace(input.DisplayName)
	if displayName == "" {
		return identity.User{}, problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"display_name": "is required"},
		)
	}
	passwordHash, err := s.hashPassword(input.Password)
	if err != nil {
		return identity.User{}, err
	}
	return identity.User{
		ID:           uuid.NewString(),
		Email:        login,
		DisplayName:  displayName,
		PasswordHash: passwordHash,
		Active:       true,
	}, nil
}

func (s *Service) hashPassword(password string) (string, error) {
	passwordHash, err := s.hasher.Hash(password)
	if err != nil {
		return "", problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"password": err.Error()},
		)
	}
	return passwordHash, nil
}

func normalizeLogin(login string) string {
	return strings.ToLower(strings.TrimSpace(login))
}

func validateID(field, value string) error {
	if _, err := uuid.Parse(value); err != nil {
		return problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{field: "must be a valid UUID"},
		)
	}
	return nil
}

func validateIDs(field string, values []string) error {
	for _, value := range values {
		if err := validateID(field, value); err != nil {
			return err
		}
	}
	return nil
}

func mapRepositoryError(err error) error {
	switch {
	case errors.Is(err, identity.ErrUserNotFound):
		return problem.New(problem.KindNotFound, "user_not_found", "user was not found")
	case errors.Is(err, identity.ErrEmailExists):
		return problem.New(problem.KindConflict, "email_exists", "a user with this login already exists")
	case errors.Is(err, identity.ErrUnknownRole):
		return problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"role_ids": "contains an unknown role"},
		)
	case errors.Is(err, identity.ErrLastOwner):
		return problem.New(problem.KindConflict, "last_owner", "the final active owner cannot be disabled or stripped of the owner role")
	case errors.Is(err, identity.ErrUsersExist):
		return problem.New(problem.KindConflict, "already_bootstrapped", "the deployment already has users")
	default:
		return problem.Wrap(err, "persist user")
	}
}
