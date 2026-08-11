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
	"resolved-server/internal/realtime"
	"resolved-server/internal/roles"
	"resolved-server/internal/users"
	"resolved-server/internal/workspaces"

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

func WithRealtime(authService *auth.Service, publisher *realtime.Publisher) Modifier {
	return func(app *fiber.App) {
		app.Get("/api/v1/ws", authService.Middleware(), publisher.Handler())
	}
}

func WithWorkspaces(authService *auth.Service, handler *workspaces.Handler) Modifier {
	return func(app *fiber.App) {
		protected := app.Group("/api/v1", authService.Middleware())

		protected.Get(
			"/workspaces",
			auth.RequirePermission(identity.PermissionWorkspacesRead),
			auth.RequirePermission(identity.PermissionCollectionsRead),
			auth.RequirePermission(identity.PermissionRequestsRead),
			handler.ListController(),
		)
		protected.Post("/workspaces", auth.RequirePermission(identity.PermissionWorkspacesCreate), handler.CreateController())
		protected.Get(
			"/workspaces/:workspace_id",
			auth.RequirePermission(identity.PermissionWorkspacesRead),
			auth.RequirePermission(identity.PermissionCollectionsRead),
			auth.RequirePermission(identity.PermissionRequestsRead),
			handler.GetController(),
		)
		protected.Patch(
			"/workspaces/:workspace_id",
			auth.RequirePermission(identity.PermissionWorkspacesUpdate),
			auth.RequirePermission(identity.PermissionCollectionsRead),
			auth.RequirePermission(identity.PermissionRequestsRead),
			handler.UpdateController(),
		)
		protected.Delete("/workspaces/:workspace_id", auth.RequirePermission(identity.PermissionWorkspacesDelete), handler.DeleteController())
		protected.Put(
			"/workspaces/:workspace_id/users",
			auth.RequirePermission(identity.PermissionWorkspacesAssignUsers),
			auth.RequirePermission(identity.PermissionCollectionsRead),
			auth.RequirePermission(identity.PermissionRequestsRead),
			handler.ReplaceUsersController(),
		)

		protected.Post("/workspaces/:workspace_id/collections", auth.RequirePermission(identity.PermissionCollectionsCreate), handler.CreateCollectionController())
		protected.Get(
			"/workspaces/:workspace_id/collections/:collection_id",
			auth.RequirePermission(identity.PermissionCollectionsRead),
			auth.RequirePermission(identity.PermissionRequestsRead),
			handler.GetCollectionController(),
		)
		protected.Patch(
			"/workspaces/:workspace_id/collections/:collection_id",
			auth.RequirePermission(identity.PermissionCollectionsUpdate),
			auth.RequirePermission(identity.PermissionRequestsRead),
			handler.UpdateCollectionController(),
		)
		protected.Delete("/workspaces/:workspace_id/collections/:collection_id", auth.RequirePermission(identity.PermissionCollectionsDelete), handler.DeleteCollectionController())
		protected.Put(
			"/workspaces/:workspace_id/collections/:collection_id/parent",
			auth.RequirePermission(identity.PermissionCollectionsUpdate),
			auth.RequirePermission(identity.PermissionRequestsRead),
			handler.MoveCollectionController(),
		)
		protected.Put(
			"/workspaces/:workspace_id/collections/:collection_id/users",
			auth.RequirePermission(identity.PermissionCollectionsAssignUsers),
			auth.RequirePermission(identity.PermissionRequestsRead),
			handler.ReplaceCollectionUsersController(),
		)

		protected.Post("/workspaces/:workspace_id/collections/:collection_id/requests", auth.RequirePermission(identity.PermissionRequestsCreate), handler.CreateSavedRequestController())
		protected.Get("/workspaces/:workspace_id/collections/:collection_id/requests/:request_id", auth.RequirePermission(identity.PermissionRequestsRead), handler.GetSavedRequestController())
		protected.Patch("/workspaces/:workspace_id/collections/:collection_id/requests/:request_id", auth.RequirePermission(identity.PermissionRequestsUpdate), handler.UpdateSavedRequestController())
		protected.Put("/workspaces/:workspace_id/collections/:collection_id/requests/:request_id/collection", auth.RequirePermission(identity.PermissionRequestsUpdate), handler.MoveSavedRequestController())
		protected.Delete("/workspaces/:workspace_id/collections/:collection_id/requests/:request_id", auth.RequirePermission(identity.PermissionRequestsDelete), handler.DeleteSavedRequestController())

		protected.Get(
			"/workspaces/:workspace_id/environments",
			auth.RequirePermission(identity.PermissionEnvironmentsRead),
			handler.ListEnvironmentsController(),
		)
		protected.Post(
			"/workspaces/:workspace_id/environments",
			auth.RequirePermission(identity.PermissionEnvironmentsCreate),
			handler.CreateEnvironmentController(),
		)
		protected.Get(
			"/workspaces/:workspace_id/environments/:environment_id",
			auth.RequirePermission(identity.PermissionEnvironmentsRead),
			handler.GetEnvironmentController(),
		)
		protected.Patch(
			"/workspaces/:workspace_id/environments/:environment_id",
			auth.RequirePermission(identity.PermissionEnvironmentsUpdate),
			handler.UpdateEnvironmentController(),
		)
		protected.Delete(
			"/workspaces/:workspace_id/environments/:environment_id",
			auth.RequirePermission(identity.PermissionEnvironmentsDelete),
			handler.DeleteEnvironmentController(),
		)
		protected.Post(
			"/workspaces/:workspace_id/environments/:environment_id/variables",
			auth.RequirePermission(identity.PermissionEnvironmentsUpdate),
			handler.CreateEnvironmentVariableController(),
		)
		protected.Patch(
			"/workspaces/:workspace_id/environments/:environment_id/variables/:variable_id",
			auth.RequirePermission(identity.PermissionEnvironmentsUpdate),
			handler.UpdateEnvironmentVariableController(),
		)
		protected.Delete(
			"/workspaces/:workspace_id/environments/:environment_id/variables/:variable_id",
			auth.RequirePermission(identity.PermissionEnvironmentsDelete),
			handler.DeleteEnvironmentVariableController(),
		)
		protected.Put(
			"/workspaces/:workspace_id/environments/:environment_id/variables/:variable_id/value",
			auth.RequirePermission(identity.PermissionEnvironmentValuesUpdate),
			handler.PutEnvironmentVariableValueController(),
		)
	}
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
