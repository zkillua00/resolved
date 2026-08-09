package server

import (
	"context"
	"fmt"
	"io"
	"os"
	"time"

	"resolved-server/internal/auth"
	"resolved-server/internal/httpkit"
	"resolved-server/internal/identity"
	"resolved-server/internal/problem"
	"resolved-server/internal/roles"
	"resolved-server/internal/users"

	"github.com/gofiber/fiber/v3"
	"github.com/gofiber/fiber/v3/middleware/helmet"
	"github.com/gofiber/fiber/v3/middleware/limiter"
	"github.com/gofiber/fiber/v3/middleware/logger"
	"github.com/gofiber/fiber/v3/middleware/recover"
	"github.com/gofiber/fiber/v3/middleware/requestid"
)

type Modifier func(app *fiber.App)

type Server struct {
	App     *fiber.App
	Address string
}

func New(address string, accessLog io.Writer, modifiers ...Modifier) *Server {
	if accessLog == nil {
		accessLog = os.Stdout
	}
	app := fiber.New(fiber.Config{
		AppName:      "Resolved collaboration server",
		ErrorHandler: httpkit.ErrorHandler,
	})
	app.Use(recover.New())
	app.Use(requestid.New())
	app.Use(logger.New(logger.Config{
		Format:   "${time} ${status} ${latency} ${method} ${path} request_id=${requestid}\n",
		TimeZone: "UTC",
		Stream:   accessLog,
	}))
	app.Use(helmet.New())

	for _, modifier := range modifiers {
		modifier(app)
	}

	return &Server{App: app, Address: address}
}

func WithIdentity(
	authService *auth.Service,
	authHandler *auth.Handler,
	usersHandler *users.Handler,
	rolesHandler *roles.Handler,
) Modifier {
	return func(app *fiber.App) {
		api := app.Group("/api/v1")
		loginLimiter := limiter.New(limiter.Config{
			Max:                    5,
			Expiration:             5 * time.Minute,
			LimiterMiddleware:      limiter.SlidingWindow{},
			SkipSuccessfulRequests: true,
			LimitReached: func(fiber.Ctx) error {
				return problem.New(problem.KindRateLimited, "rate_limited", "too many login attempts; try again later")
			},
		})
		api.Post("/auth/login", loginLimiter, authHandler.LoginController())

		protected := api.Group("/", authService.Middleware())
		protected.Get("/auth/me", authHandler.MeController())
		protected.Post("/auth/logout", authHandler.LogoutController())

		protected.Get("/users", auth.RequirePermission(identity.PermissionUsersRead), usersHandler.ListController())
		protected.Post("/users", auth.RequirePermission(identity.PermissionUsersCreate), usersHandler.CreateController())
		protected.Get("/users/:id", auth.RequirePermission(identity.PermissionUsersRead), usersHandler.GetController())
		protected.Patch("/users/:id", auth.RequirePermission(identity.PermissionUsersUpdate), usersHandler.UpdateController())
		protected.Put("/users/:id/roles", auth.RequirePermission(identity.PermissionUsersAssignRoles), usersHandler.ReplaceRolesController())

		protected.Get("/roles", auth.RequirePermission(identity.PermissionRolesRead), rolesHandler.ListController())
		protected.Post("/roles", auth.RequirePermission(identity.PermissionRolesCreate), rolesHandler.CreateController())
		protected.Get("/roles/:id", auth.RequirePermission(identity.PermissionRolesRead), rolesHandler.GetController())
		protected.Patch("/roles/:id", auth.RequirePermission(identity.PermissionRolesUpdate), rolesHandler.UpdateController())
		protected.Put("/roles/:id/permissions", auth.RequirePermission(identity.PermissionRolesAssignPermissions), rolesHandler.ReplacePermissionsController())
		protected.Get("/permissions", auth.RequirePermission(identity.PermissionPermissionsRead), rolesHandler.ListPermissionsController())
	}
}

func (s *Server) Start() error {
	if err := s.App.Listen(s.Address); err != nil {
		return fmt.Errorf("listen on %s: %w", s.Address, err)
	}
	return nil
}

func (s *Server) Shutdown(ctx context.Context) error {
	return s.App.ShutdownWithContext(ctx)
}
