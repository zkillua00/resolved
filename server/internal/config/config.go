package config

import (
	"fmt"
	"os"
	"strings"
	"time"
)

const (
	defaultAddress    = "127.0.0.1:8787"
	defaultDriver     = "sqlite"
	defaultSQLiteDSN  = "./data/resolved-server.db?_busy_timeout=5000&_journal_mode=WAL&_foreign_keys=on"
	defaultSessionTTL = 24 * time.Hour
)

type Config struct {
	Address          string
	Database         Database
	SessionTTL       time.Duration
	EncryptionSecret string
}

type Database struct {
	Driver string
	DSN    string
}

func Load() (Config, error) {
	driver, err := NormalizeDriver(envOrDefault("RESOLVED_DATABASE_DRIVER", defaultDriver))
	if err != nil {
		return Config{}, err
	}

	dsn := strings.TrimSpace(os.Getenv("RESOLVED_DATABASE_DSN"))
	if dsn == "" {
		if driver != defaultDriver {
			return Config{}, fmt.Errorf("RESOLVED_DATABASE_DSN is required for %s", driver)
		}
		dsn = defaultSQLiteDSN
	}

	ttl, err := time.ParseDuration(envOrDefault("RESOLVED_SESSION_TTL", defaultSessionTTL.String()))
	if err != nil || ttl <= 0 {
		return Config{}, fmt.Errorf("RESOLVED_SESSION_TTL must be a positive duration")
	}

	address := strings.TrimSpace(envOrDefault("RESOLVED_SERVER_ADDRESS", defaultAddress))
	if address == "" {
		return Config{}, fmt.Errorf("RESOLVED_SERVER_ADDRESS cannot be empty")
	}

	encryptionSecret := os.Getenv("RESOLVED_ENCRYPTION_SECRET")
	if len([]byte(encryptionSecret)) < 32 {
		return Config{}, fmt.Errorf("RESOLVED_ENCRYPTION_SECRET must contain at least 32 bytes")
	}

	return Config{
		Address: address,
		Database: Database{
			Driver: driver,
			DSN:    dsn,
		},
		SessionTTL:       ttl,
		EncryptionSecret: encryptionSecret,
	}, nil
}

func NormalizeDriver(driver string) (string, error) {
	switch strings.ToLower(strings.TrimSpace(driver)) {
	case "sqlite", "sqlite3":
		return "sqlite", nil
	case "postgres", "postgresql":
		return "postgres", nil
	case "mysql":
		return "mysql", nil
	case "sqlserver", "mssql":
		return "sqlserver", nil
	default:
		return "", fmt.Errorf("unsupported database driver %q", driver)
	}
}

func envOrDefault(key, fallback string) string {
	if value, ok := os.LookupEnv(key); ok && strings.TrimSpace(value) != "" {
		return value
	}
	return fallback
}
