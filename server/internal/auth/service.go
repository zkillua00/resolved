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
	repository        *identity.Repository
	hasher            *security.PasswordHasher
	environmentCipher *security.EnvironmentCipher
	sessionKeys       *security.SessionEnvironmentKeys
	sessionTTL        time.Duration
	dummyHash         string
}

type LoginResult struct {
	Token     string
	ExpiresAt time.Time
	User      identity.User
}

type Principal struct {
	User           identity.User
	permissions    map[string]struct{}
	environmentKey []byte
}

func NewService(
	repository *identity.Repository,
	hasher *security.PasswordHasher,
	environmentCipher *security.EnvironmentCipher,
	sessionKeys *security.SessionEnvironmentKeys,
	sessionTTL time.Duration,
) (*Service, error) {
	if environmentCipher == nil {
		return nil, errors.New("environment cipher is required")
	}
	if sessionKeys == nil {
		return nil, errors.New("session environment keys are required")
	}
	dummyHash, err := hasher.Hash(dummyPassword)
	if err != nil {
		return nil, err
	}
	return &Service{
		repository:        repository,
		hasher:            hasher,
		environmentCipher: environmentCipher,
		sessionKeys:       sessionKeys,
		sessionTTL:        sessionTTL,
		dummyHash:         dummyHash,
	}, nil
}

func (s *Service) Login(ctx context.Context, login, password string) (LoginResult, error) {
	login = normalizeLogin(login)
	user, err := s.repository.FindUserByEmail(ctx, login)
	if err != nil {
		if errors.Is(err, identity.ErrUserNotFound) {
			_, _ = s.hasher.Verify(s.dummyHash, password)
			s.consumeEnvironmentDerivation("00000000-0000-0000-0000-000000000000", password)
			return LoginResult{}, invalidCredentials()
		}
		return LoginResult{}, problem.Wrap(err, "find login user")
	}

	valid, err := s.hasher.Verify(user.PasswordHash, password)
	if err != nil {
		return LoginResult{}, problem.Wrap(err, "verify stored password")
	}
	if !valid || !user.Active {
		s.consumeEnvironmentDerivation(user.ID, password)
		return LoginResult{}, invalidCredentials()
	}
	verifiedPasswordHash := user.PasswordHash
	user, err = s.repository.GetUser(ctx, user.ID)
	if err != nil {
		return LoginResult{}, problem.Wrap(err, "load authenticated user")
	}
	if !user.Active || user.PasswordHash != verifiedPasswordHash {
		s.consumeEnvironmentDerivation(user.ID, password)
		return LoginResult{}, invalidCredentials()
	}
	environmentKey, err := s.environmentCipher.DeriveUserKey(user.ID, password)
	if err != nil {
		return LoginResult{}, problem.Wrap(err, "derive login environment key")
	}
	defer clear(environmentKey)

	token, err := security.NewSessionToken()
	if err != nil {
		return LoginResult{}, problem.Wrap(err, "create session token")
	}
	createdAt := time.Now().UTC()
	expiresAt := createdAt.Add(s.sessionTTL)
	sessionID := uuid.NewString()
	tokenHash := security.DigestSessionToken(token)
	session := identity.Session{
		ID:        sessionID,
		TokenHash: tokenHash,
		UserID:    user.ID,
		ExpiresAt: expiresAt,
		CreatedAt: createdAt,
	}
	if err := s.repository.CreateSession(ctx, session); err != nil {
		return LoginResult{}, problem.Wrap(err, "persist login session")
	}
	if err := s.sessionKeys.Put(tokenHash, user.ID, user.PasswordHash, environmentKey, expiresAt); err != nil {
		_ = s.repository.DeleteSessionByHash(ctx, tokenHash)
		return LoginResult{}, problem.Wrap(err, "retain login environment key")
	}

	return LoginResult{Token: token, ExpiresAt: expiresAt, User: user}, nil
}

func (s *Service) consumeEnvironmentDerivation(userID, password string) {
	key, err := s.environmentCipher.DeriveUserKey(userID, password)
	if err == nil {
		clear(key)
	}
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
		s.sessionKeys.Delete(hash)
		_ = s.repository.DeleteSessionByHash(ctx, hash)
		return nil, unauthorized()
	}
	environmentKey, ok := s.sessionKeys.Get(hash, session.UserID, session.User.PasswordHash)
	if !ok {
		_ = s.repository.DeleteSessionByHash(ctx, hash)
		return nil, unauthorized()
	}

	permissions := make(map[string]struct{})
	for _, role := range session.User.Roles {
		for _, permission := range role.Permissions {
			permissions[permission.Key] = struct{}{}
		}
	}
	return &Principal{
		User:           session.User,
		permissions:    permissions,
		environmentKey: environmentKey,
	}, nil
}

func (s *Service) Logout(ctx context.Context, token string) error {
	if token == "" {
		return unauthorized()
	}
	hash := security.DigestSessionToken(token)
	s.sessionKeys.Delete(hash)
	if err := s.repository.DeleteSessionByHash(ctx, hash); err != nil {
		return problem.Wrap(err, "revoke login session")
	}
	return nil
}

func (p *Principal) HasPermission(permission string) bool {
	_, ok := p.permissions[permission]
	return ok
}

func (p *Principal) HasRole(roleID string) bool {
	for _, role := range p.User.Roles {
		if role.ID == roleID {
			return true
		}
	}
	return false
}

func (p *Principal) EnvironmentKey() []byte {
	if p == nil {
		return nil
	}
	return append([]byte(nil), p.environmentKey...)
}

func normalizeLogin(login string) string {
	return strings.ToLower(strings.TrimSpace(login))
}

func invalidCredentials() error {
	return problem.New(problem.KindUnauthorized, "invalid_credentials", "login or password is incorrect")
}

func unauthorized() error {
	return problem.New(problem.KindUnauthorized, "unauthorized", "a valid bearer token is required")
}
