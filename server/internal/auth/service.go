package auth

import (
	"context"
	"errors"
	"strings"
	"time"

	"resolved-server/internal/identity"
	"resolved-server/internal/problem"
	"resolved-server/internal/security"

	"github.com/google/uuid"
	"gorm.io/gorm"
)

const dummyPassword = "this password exists only for timing"

type Service struct {
	repository *identity.Repository
	hasher     *security.PasswordHasher
	sessionTTL time.Duration
	dummyHash  string
}

type LoginResult struct {
	Token     string
	ExpiresAt time.Time
	User      identity.User
}

type Principal struct {
	User        identity.User
	permissions map[string]struct{}
}

func NewService(
	repository *identity.Repository,
	hasher *security.PasswordHasher,
	sessionTTL time.Duration,
) (*Service, error) {
	dummyHash, err := hasher.Hash(dummyPassword)
	if err != nil {
		return nil, err
	}
	return &Service{
		repository: repository,
		hasher:     hasher,
		sessionTTL: sessionTTL,
		dummyHash:  dummyHash,
	}, nil
}

func (s *Service) Login(ctx context.Context, email, password string) (LoginResult, error) {
	email = normalizeEmail(email)
	user, err := s.repository.FindUserByEmail(ctx, email)
	if err != nil {
		if errors.Is(err, identity.ErrUserNotFound) {
			_, _ = s.hasher.Verify(s.dummyHash, password)
			return LoginResult{}, invalidCredentials()
		}
		return LoginResult{}, problem.Wrap(err, "find login user")
	}

	valid, err := s.hasher.Verify(user.PasswordHash, password)
	if err != nil {
		return LoginResult{}, problem.Wrap(err, "verify stored password")
	}
	if !valid || !user.Active {
		return LoginResult{}, invalidCredentials()
	}
	user, err = s.repository.GetUser(ctx, user.ID)
	if err != nil {
		return LoginResult{}, problem.Wrap(err, "load authenticated user")
	}

	token, err := security.NewSessionToken()
	if err != nil {
		return LoginResult{}, problem.Wrap(err, "create session token")
	}
	createdAt := time.Now().UTC()
	expiresAt := createdAt.Add(s.sessionTTL)
	session := identity.Session{
		ID:        uuid.NewString(),
		TokenHash: security.DigestSessionToken(token),
		UserID:    user.ID,
		ExpiresAt: expiresAt,
		CreatedAt: createdAt,
	}
	if err := s.repository.CreateSession(ctx, session); err != nil {
		return LoginResult{}, problem.Wrap(err, "persist login session")
	}

	return LoginResult{Token: token, ExpiresAt: expiresAt, User: user}, nil
}

func (s *Service) Authenticate(ctx context.Context, token string) (*Principal, error) {
	if token == "" {
		return nil, unauthorized()
	}
	hash := security.DigestSessionToken(token)
	session, err := s.repository.FindSessionByHash(ctx, hash)
	if err != nil {
		if errors.Is(err, gorm.ErrRecordNotFound) {
			return nil, unauthorized()
		}
		return nil, problem.Wrap(err, "load login session")
	}
	if !session.ExpiresAt.After(time.Now().UTC()) || !session.User.Active {
		_ = s.repository.DeleteSessionByHash(ctx, hash)
		return nil, unauthorized()
	}

	permissions := make(map[string]struct{})
	for _, role := range session.User.Roles {
		for _, permission := range role.Permissions {
			permissions[permission.Key] = struct{}{}
		}
	}
	return &Principal{User: session.User, permissions: permissions}, nil
}

func (s *Service) Logout(ctx context.Context, token string) error {
	if token == "" {
		return unauthorized()
	}
	if err := s.repository.DeleteSessionByHash(ctx, security.DigestSessionToken(token)); err != nil {
		return problem.Wrap(err, "revoke login session")
	}
	return nil
}

func (p *Principal) HasPermission(permission string) bool {
	_, ok := p.permissions[permission]
	return ok
}

func normalizeEmail(email string) string {
	return strings.ToLower(strings.TrimSpace(email))
}

func invalidCredentials() error {
	return problem.New(problem.KindUnauthorized, "invalid_credentials", "email or password is incorrect")
}

func unauthorized() error {
	return problem.New(problem.KindUnauthorized, "unauthorized", "a valid bearer token is required")
}
