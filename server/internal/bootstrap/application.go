package bootstrap

import (
	"context"
	"database/sql"
	"fmt"
	"io"

	"resolved-server/internal/auth"
	"resolved-server/internal/config"
	"resolved-server/internal/database"
	"resolved-server/internal/identity"
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
	)
	rolesService := roles.NewService(repository)
	workspaceRepository := workspaces.NewRepository(db)
	workspacesService := workspaces.NewService(workspaceRepository, environmentCipher)

	authHandler := auth.NewHandler(authService)
	usersHandler := users.NewHandler(usersService)
	rolesHandler := roles.NewHandler(rolesService)
	workspacesHandler := workspaces.NewHandler(workspacesService)
	httpServer := server.New(
		cfg.Address,
		accessLog,
		server.WithIdentity(authService, authHandler, usersHandler, rolesHandler),
		server.WithWorkspaces(authService, workspacesHandler),
	)

	return &Application{
		Server:      httpServer,
		Users:       usersService,
		sqlDatabase: sqlDatabase,
	}, nil
}

func (a *Application) Close() error {
	return a.sqlDatabase.Close()
}
