package users

import (
	"context"
	"errors"
	"fmt"
	"strings"

	"resolved-server/internal/identity"
	"resolved-server/internal/problem"
	"resolved-server/internal/resourceevents"
	"resolved-server/internal/security"

	"github.com/google/uuid"
	"gorm.io/gorm"
)

type Service struct {
	repository          *identity.Repository
	hasher              *security.PasswordHasher
	environmentCipher   *security.EnvironmentCipher
	sessionKeys         *security.SessionEnvironmentKeys
	firstOwnerSetup     identity.FirstOwnerSetup
	passwordChangeSetup PasswordChangeSetup
	events              resourceevents.Emitter
}

type ServiceOption func(*Service)

type PasswordChangeSetup func(context.Context, *gorm.DB, string, []byte, []byte) error

type CreateInput struct {
	Email           string
	DisplayName     string
	Password        string
	RoleIDs         []string
	CreatedByUserID *string
}

type UpdateInput struct {
	Email       *string
	DisplayName *string
	Password    *string
	Active      *bool
}

func WithFirstOwnerSetup(setup identity.FirstOwnerSetup) ServiceOption {
	return func(service *Service) {
		service.firstOwnerSetup = setup
	}
}

func WithPasswordChangeSetup(setup PasswordChangeSetup) ServiceOption {
	return func(service *Service) {
		service.passwordChangeSetup = setup
	}
}

func WithEvents(events resourceevents.Emitter) ServiceOption {
	return func(service *Service) {
		service.events = events
	}
}

func NewService(
	repository *identity.Repository,
	hasher *security.PasswordHasher,
	environmentCipher *security.EnvironmentCipher,
	sessionKeys *security.SessionEnvironmentKeys,
	options ...ServiceOption,
) *Service {
	service := &Service{
		repository:        repository,
		hasher:            hasher,
		environmentCipher: environmentCipher,
		sessionKeys:       sessionKeys,
	}
	for _, option := range options {
		option(service)
	}
	return service
}

func (s *Service) BootstrapOwner(ctx context.Context, input CreateInput) (identity.User, error) {
	user, err := s.newUser(input)
	if err != nil {
		return identity.User{}, err
	}
	created, err := s.repository.CreateFirstOwner(ctx, user, s.firstOwnerSetup)
	if err != nil {
		return identity.User{}, mapRepositoryError(err)
	}
	s.publishUserChange(resourceevents.ActionCreated, created.ID)
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
	s.publishUserChange(resourceevents.ActionCreated, created.ID)
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
	var beforePasswordChange identity.BeforePasswordChange
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
		beforePasswordChange = func(
			ctx context.Context,
			tx *gorm.DB,
			user identity.User,
			_ *identity.UserChanges,
		) error {
			oldKey, ok := s.sessionKeys.AnyForUser(user.ID, user.PasswordHash)
			if ok {
				defer clear(oldKey)
			}

			newKey, err := s.environmentCipher.DeriveUserKey(user.ID, *input.Password)
			if err != nil {
				return err
			}
			defer clear(newKey)
			if s.passwordChangeSetup == nil {
				return errors.New("environment value password-change setup is required")
			}
			if err := s.passwordChangeSetup(ctx, tx, user.ID, oldKey, newKey); err != nil {
				return fmt.Errorf("re-encrypt environment values: %w", err)
			}
			return nil
		}
	}

	user, err := s.repository.UpdateUser(ctx, id, changes, beforePasswordChange)
	if err != nil {
		return identity.User{}, mapRepositoryError(err)
	}
	if input.Password != nil || input.Active != nil && !*input.Active {
		s.sessionKeys.DeleteUser(id)
	}
	s.publishUserChange(resourceevents.ActionUpdated, user.ID)
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
	s.publishUserChange(resourceevents.ActionUpdated, user.ID)
	return user, nil
}

func (s *Service) publishUserChange(action resourceevents.Action, userID string) {
	resourceevents.Emit(s.events, resourceevents.Change{
		Resource:   resourceevents.ResourceUser,
		Action:     action,
		ResourceID: userID,
		Audience: resourceevents.Audience{
			UserIDs:        []string{userID},
			PermissionKeys: []string{identity.PermissionUsersRead, identity.PermissionRolesRead},
		},
	})
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
	userID := uuid.NewString()
	return identity.User{
		ID:              userID,
		Email:           login,
		DisplayName:     displayName,
		PasswordHash:    passwordHash,
		Active:          true,
		CreatedByUserID: input.CreatedByUserID,
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
	case errors.Is(err, security.ErrEnvironmentKeyUnavailable):
		return problem.New(
			problem.KindConflict,
			"environment_key_unavailable",
			"the user's active login is required to preserve encrypted environment values during a password change",
		)
	default:
		return problem.Wrap(err, "persist user")
	}
}
