package bootstrap

import (
	"context"
	"database/sql"
	"fmt"
	"io"

	"resolved-server/eventsystem"
	"resolved-server/internal/activitylog"
	"resolved-server/internal/auth"
	"resolved-server/internal/config"
	"resolved-server/internal/database"
	"resolved-server/internal/identity"
	"resolved-server/internal/realtime"
	"resolved-server/internal/requestproxy"
	"resolved-server/internal/roles"
	"resolved-server/internal/security"
	"resolved-server/internal/server"
	"resolved-server/internal/sharedhistory"
	"resolved-server/internal/users"
	"resolved-server/internal/workspaces"

	"gorm.io/gorm"
)

type Application struct {
	Server      *server.Server
	Users       *users.Service
	events      *eventsystem.EventListener
	dataCipher  *security.DataCipher
	sqlDatabase *sql.DB
}

func New(cfg config.Config, accessLog io.Writer) (*Application, error) {
	var dataKeyProvider security.KeyProvider
	var err error
	switch cfg.DataEncryption.Provider {
	case "static":
		dataKeyProvider, err = security.NewStaticKeyProvider(
			cfg.DataEncryption.KeyID,
			cfg.DataEncryption.EncodedKey,
		)
	case "vault":
		dataKeyProvider, err = security.NewVaultTransitKeyProvider(
			cfg.DataEncryption.VaultAddress,
			cfg.DataEncryption.VaultToken,
			cfg.DataEncryption.VaultNamespace,
			cfg.DataEncryption.VaultMount,
			cfg.DataEncryption.VaultKeyName,
		)
	default:
		err = fmt.Errorf("unsupported data key provider %q", cfg.DataEncryption.Provider)
	}
	if err != nil {
		return nil, fmt.Errorf("initialize data key provider: %w", err)
	}
	dataCipher, err := security.NewDataCipher(dataKeyProvider)
	if err != nil {
		return nil, fmt.Errorf("initialize server data encryption: %w", err)
	}
	db, err := database.Open(cfg.Database)
	if err != nil {
		dataCipher.Close()
		return nil, err
	}
	sqlDatabase, err := db.DB()
	if err != nil {
		dataCipher.Close()
		return nil, fmt.Errorf("access database connection: %w", err)
	}
	closeOnError := func() {
		dataCipher.Close()
		_ = sqlDatabase.Close()
	}

	if err := database.MigrateAndSeed(db); err != nil {
		closeOnError()
		return nil, err
	}
	var events *eventsystem.EventListener
	closeCipherOnError := func() {
		if events != nil {
			events.Stop()
		}
		closeOnError()
	}
	if err := workspaces.EnsureInitialWorkspace(context.Background(), db); err != nil {
		closeCipherOnError()
		return nil, fmt.Errorf("initialize default workspace: %w", err)
	}

	repository := identity.NewRepository(db, dataCipher)
	if err := repository.EncryptLegacyRoles(context.Background()); err != nil {
		closeCipherOnError()
		return nil, fmt.Errorf("encrypt existing role profiles: %w", err)
	}
	if err := repository.EncryptLegacyUsers(context.Background()); err != nil {
		closeCipherOnError()
		return nil, fmt.Errorf("encrypt existing user profiles: %w", err)
	}
	if err := repository.DeleteAllSessions(context.Background()); err != nil {
		closeCipherOnError()
		return nil, fmt.Errorf("invalidate sessions from previous server process: %w", err)
	}
	passwordParams := security.DefaultPasswordParams()
	hasher := security.NewPasswordHasher(passwordParams)
	environmentCipher, err := security.NewEnvironmentCipher(cfg.EncryptionSecret, passwordParams)
	if err != nil {
		closeCipherOnError()
		return nil, fmt.Errorf("initialize environment encryption: %w", err)
	}
	sessionKeys := security.NewSessionEnvironmentKeys()
	authService, err := auth.NewService(repository, hasher, environmentCipher, sessionKeys, cfg.SessionTTL)
	if err != nil {
		closeCipherOnError()
		return nil, fmt.Errorf("initialize authentication: %w", err)
	}
	events = eventsystem.NewEventListener()
	for range 3 {
		events.StartNewEventLoop()
	}
	activityRepository := activitylog.NewRepository(db, dataCipher)
	recordedEvents := activitylog.NewRecorder(activityRepository, events)
	usersService := users.NewService(
		repository,
		hasher,
		environmentCipher,
		sessionKeys,
		users.WithFirstOwnerSetup(func(ctx context.Context, tx *gorm.DB, owner identity.User) error {
			return workspaces.SetupFirstOwnerWorkspaceEncrypted(ctx, tx, owner, dataCipher)
		}),
		users.WithPasswordChangeSetup(func(
			ctx context.Context,
			tx *gorm.DB,
			userID string,
			oldKey, newKey []byte,
		) error {
			return workspaces.RekeyEnvironmentVariableValues(ctx, tx, environmentCipher, userID, oldKey, newKey)
		}),
		users.WithEvents(recordedEvents),
	)
	rolesService := roles.NewService(repository, roles.WithEvents(recordedEvents))
	workspaceRepository := workspaces.NewRepository(db, dataCipher)
	environmentRepository := workspaces.NewEnvironmentRepository(workspaceRepository)
	sharedHistoryRepository := sharedhistory.NewRepository(db, dataCipher)
	settingsRepository := requestproxy.NewSettingsRepository(db, dataCipher)
	proxyRepository := requestproxy.NewProxyRepository(db, dataCipher)
	if err := workspaceRepository.EncryptLegacyResourceNames(context.Background()); err != nil {
		closeCipherOnError()
		return nil, fmt.Errorf("encrypt existing workspace resource names: %w", err)
	}
	if err := environmentRepository.EncryptLegacyEnvironmentVariableKeys(context.Background()); err != nil {
		closeCipherOnError()
		return nil, fmt.Errorf("encrypt existing environment variable keys: %w", err)
	}
	if err := workspaceRepository.EncryptLegacySavedRequests(context.Background()); err != nil {
		closeCipherOnError()
		return nil, fmt.Errorf("encrypt existing saved requests: %w", err)
	}
	if err := sharedHistoryRepository.EncryptLegacyEntries(context.Background()); err != nil {
		closeCipherOnError()
		return nil, fmt.Errorf("encrypt existing shared history: %w", err)
	}
	if err := activityRepository.EncryptLegacyEntries(context.Background()); err != nil {
		closeCipherOnError()
		return nil, fmt.Errorf("encrypt existing activity logs: %w", err)
	}
	if err := proxyRepository.AdoptLegacyOverrides(context.Background(), settingsRepository); err != nil {
		closeCipherOnError()
		return nil, fmt.Errorf("adopt existing request hostname overrides: %w", err)
	}
	if err := database.FinalizeDataEncryptionMigration(context.Background(), db, cfg.Database); err != nil {
		closeCipherOnError()
		return nil, err
	}
	workspacesService := workspaces.NewService(
		workspaceRepository,
		environmentCipher,
		workspaces.WithEvents(recordedEvents),
	)
	realtimePublisher := realtime.New(events)

	authHandler := auth.NewHandler(authService)
	usersHandler := users.NewHandler(usersService)
	rolesHandler := roles.NewHandler(rolesService)
	workspacesHandler := workspaces.NewHandler(workspacesService)
	requestProxyHandler := requestproxy.NewHandler(requestproxy.NewService(
		workspacesService,
		settingsRepository,
		proxyRepository,
		requestproxy.WithEvents(recordedEvents),
	))
	sharedHistoryHandler := sharedhistory.NewHandler(sharedhistory.NewService(
		sharedHistoryRepository,
		workspacesService,
		sharedhistory.WithEvents(events),
	))
	activityHandler := activitylog.NewHandler(activitylog.NewService(
		activityRepository,
		workspacesService,
	))
	httpServer := server.New(
		cfg.Address,
		accessLog,
		server.WithIdentity(authService, authHandler, usersHandler, rolesHandler),
		server.WithWorkspaces(authService, workspacesHandler),
		server.WithRequestProxy(authService, requestProxyHandler),
		server.WithSharedHistory(authService, sharedHistoryHandler),
		server.WithActivityLogs(authService, activityHandler),
		server.WithRealtime(authService, realtimePublisher),
	)

	return &Application{
		Server:      httpServer,
		Users:       usersService,
		events:      events,
		dataCipher:  dataCipher,
		sqlDatabase: sqlDatabase,
	}, nil
}

func (a *Application) Close() error {
	if a.events != nil {
		a.events.Stop()
	}
	if a.dataCipher != nil {
		a.dataCipher.Close()
	}
	return a.sqlDatabase.Close()
}
