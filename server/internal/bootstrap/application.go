package bootstrap

import (
	"context"
	"database/sql"
	"fmt"
	"io"

	"resolved-server/eventsystem"
	"resolved-server/internal/auth"
	"resolved-server/internal/config"
	"resolved-server/internal/database"
	"resolved-server/internal/identity"
	"resolved-server/internal/realtime"
	"resolved-server/internal/requestproxy"
	"resolved-server/internal/roles"
	"resolved-server/internal/security"
	"resolved-server/internal/server"
	"resolved-server/internal/users"
	"resolved-server/internal/workspaces"

	"gorm.io/gorm"
)

type Application struct {
	Server      *server.Server
	Users       *users.Service
	events      *eventsystem.EventListener
	sqlDatabase *sql.DB
}

func New(cfg config.Config, accessLog io.Writer) (*Application, error) {
	db, err := database.Open(cfg.Database)
	if err != nil {
		return nil, err
	}
	sqlDatabase, err := db.DB()
	if err != nil {
		return nil, fmt.Errorf("access database connection: %w", err)
	}
	closeOnError := func() {
		_ = sqlDatabase.Close()
	}

	if err := database.MigrateAndSeed(db); err != nil {
		closeOnError()
		return nil, err
	}
	if err := workspaces.EnsureInitialWorkspace(context.Background(), db); err != nil {
		closeOnError()
		return nil, fmt.Errorf("initialize default workspace: %w", err)
	}

	repository := identity.NewRepository(db)
	if err := repository.DeleteAllSessions(context.Background()); err != nil {
		closeOnError()
		return nil, fmt.Errorf("invalidate sessions from previous server process: %w", err)
	}
	passwordParams := security.DefaultPasswordParams()
	hasher := security.NewPasswordHasher(passwordParams)
	environmentCipher, err := security.NewEnvironmentCipher(cfg.EncryptionSecret, passwordParams)
	if err != nil {
		closeOnError()
		return nil, fmt.Errorf("initialize environment encryption: %w", err)
	}
	sessionKeys := security.NewSessionEnvironmentKeys()
	authService, err := auth.NewService(repository, hasher, environmentCipher, sessionKeys, cfg.SessionTTL)
	if err != nil {
		closeOnError()
		return nil, fmt.Errorf("initialize authentication: %w", err)
	}
	events := eventsystem.NewEventListener()
	for range 3 {
		events.StartNewEventLoop()
	}
	usersService := users.NewService(
		repository,
		hasher,
		environmentCipher,
		sessionKeys,
		users.WithFirstOwnerSetup(workspaces.SetupFirstOwnerWorkspace),
		users.WithPasswordChangeSetup(func(
			ctx context.Context,
			tx *gorm.DB,
			userID string,
			oldKey, newKey []byte,
		) error {
			return workspaces.RekeyEnvironmentVariableValues(ctx, tx, environmentCipher, userID, oldKey, newKey)
		}),
		users.WithEvents(events),
	)
	rolesService := roles.NewService(repository, roles.WithEvents(events))
	workspaceRepository := workspaces.NewRepository(db)
	workspacesService := workspaces.NewService(
		workspaceRepository,
		environmentCipher,
		workspaces.WithEvents(events),
	)
	realtimePublisher := realtime.New(events)

	authHandler := auth.NewHandler(authService)
	usersHandler := users.NewHandler(usersService)
	rolesHandler := roles.NewHandler(rolesService)
	workspacesHandler := workspaces.NewHandler(workspacesService)
	requestProxyHandler := requestproxy.NewHandler(requestproxy.NewService(
		workspacesService,
		requestproxy.NewSettingsRepository(db),
	))
	httpServer := server.New(
		cfg.Address,
		accessLog,
		server.WithIdentity(authService, authHandler, usersHandler, rolesHandler),
		server.WithWorkspaces(authService, workspacesHandler),
		server.WithRequestProxy(authService, requestProxyHandler),
		server.WithRealtime(authService, realtimePublisher),
	)

	return &Application{
		Server:      httpServer,
		Users:       usersService,
		events:      events,
		sqlDatabase: sqlDatabase,
	}, nil
}

func (a *Application) Close() error {
	if a.events != nil {
		a.events.Stop()
	}
	return a.sqlDatabase.Close()
}
