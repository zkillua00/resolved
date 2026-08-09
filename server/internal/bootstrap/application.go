package bootstrap

import (
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

	repository := identity.NewRepository(db)
	hasher := security.NewPasswordHasher(security.DefaultPasswordParams())
	authService, err := auth.NewService(repository, hasher, cfg.SessionTTL)
	if err != nil {
		closeOnError()
		return nil, fmt.Errorf("initialize authentication: %w", err)
	}
	usersService := users.NewService(repository, hasher)
	rolesService := roles.NewService(repository)

	authHandler := auth.NewHandler(authService)
	usersHandler := users.NewHandler(usersService)
	rolesHandler := roles.NewHandler(rolesService)
	httpServer := server.New(
		cfg.Address,
		accessLog,
		server.WithIdentity(authService, authHandler, usersHandler, rolesHandler),
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
