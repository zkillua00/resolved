package auth

import (
	"strings"

	"resolved-server/internal/problem"

	"github.com/gofiber/fiber/v3"
)

type principalContextKey struct{}
type tokenContextKey struct{}

var (
	principalKey principalContextKey
	tokenKey     tokenContextKey
)

func (s *Service) Middleware() fiber.Handler {
	return func(c fiber.Ctx) error {
		token, ok := bearerToken(c.Get(fiber.HeaderAuthorization))
		if !ok {
			return unauthorized()
		}
		principal, err := s.Authenticate(c.Context(), token)
		if err != nil {
			return err
		}
		c.Locals(principalKey, principal)
		c.Locals(tokenKey, token)
		return c.Next()
	}
}

func RequirePermission(permission string) fiber.Handler {
	return func(c fiber.Ctx) error {
		principal := PrincipalFromContext(c)
		if principal == nil || !principal.HasPermission(permission) {
			return problem.New(problem.KindForbidden, "forbidden", "the account does not have the required permission")
		}
		return c.Next()
	}
}

func PrincipalFromContext(c fiber.Ctx) *Principal {
	return fiber.Locals[*Principal](c, principalKey)
}

func TokenFromContext(c fiber.Ctx) string {
	return fiber.Locals[string](c, tokenKey)
}

func bearerToken(header string) (string, bool) {
	parts := strings.Fields(header)
	if len(parts) != 2 || !strings.EqualFold(parts[0], "Bearer") || parts[1] == "" {
		return "", false
	}
	return parts[1], true
}
