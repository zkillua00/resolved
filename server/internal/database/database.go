package database

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"resolved-server/internal/config"

	"gorm.io/driver/mysql"
	"gorm.io/driver/postgres"
	"gorm.io/driver/sqlite"
	"gorm.io/driver/sqlserver"
	"gorm.io/gorm"
)

func Open(cfg config.Database) (*gorm.DB, error) {
	dialector, err := Dialector(cfg)
	if err != nil {
		return nil, err
	}

	if cfg.Driver == "sqlite" {
		if err := prepareSQLite(cfg.DSN); err != nil {
			return nil, err
		}
	}

	db, err := gorm.Open(dialector, &gorm.Config{
		DisableAutomaticPing: true,
		NowFunc:              func() time.Time { return time.Now().UTC() },
		TranslateError:       true,
	})
	if err != nil {
		return nil, fmt.Errorf("open %s database: %w", cfg.Driver, err)
	}

	sqlDB, err := db.DB()
	if err != nil {
		return nil, fmt.Errorf("access database connection: %w", err)
	}
	if cfg.Driver == "sqlite" {
		sqlDB.SetMaxOpenConns(1)
	} else {
		sqlDB.SetMaxIdleConns(5)
		sqlDB.SetMaxOpenConns(20)
	}
	pingContext, cancelPing := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancelPing()
	if err := sqlDB.PingContext(pingContext); err != nil {
		_ = sqlDB.Close()
		return nil, fmt.Errorf("ping %s database: %w", cfg.Driver, err)
	}

	if cfg.Driver == "sqlite" {
		secureSQLiteFile(cfg.DSN)
	}

	return db, nil
}

func Dialector(cfg config.Database) (gorm.Dialector, error) {
	switch cfg.Driver {
	case "sqlite":
		return sqlite.Open(cfg.DSN), nil
	case "postgres":
		return postgres.Open(cfg.DSN), nil
	case "mysql":
		return mysql.Open(cfg.DSN), nil
	case "sqlserver":
		return sqlserver.Open(cfg.DSN), nil
	default:
		return nil, fmt.Errorf("unsupported database driver %q", cfg.Driver)
	}
}

func prepareSQLite(dsn string) error {
	path := sqlitePath(dsn)
	if path == "" {
		return nil
	}
	directory := filepath.Dir(path)
	if directory == "." {
		return nil
	}
	if err := os.MkdirAll(directory, 0o700); err != nil {
		return fmt.Errorf("create SQLite data directory: %w", err)
	}
	return nil
}

func secureSQLiteFile(dsn string) {
	path := sqlitePath(dsn)
	if path != "" {
		_ = os.Chmod(path, 0o600)
	}
}

func sqlitePath(dsn string) string {
	trimmed := strings.TrimSpace(dsn)
	if strings.Contains(strings.ToLower(trimmed), "mode=memory") {
		return ""
	}
	path := strings.TrimSpace(strings.SplitN(trimmed, "?", 2)[0])
	if path == "" || path == ":memory:" {
		return ""
	}
	if strings.HasPrefix(path, "file:") {
		path = strings.TrimPrefix(path, "file:")
	}
	if path == ":memory:" {
		return ""
	}
	return path
}
