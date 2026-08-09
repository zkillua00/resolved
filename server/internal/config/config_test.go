package config

import (
	"testing"
	"time"
)

func TestLoadDefaultsToLocalSQLite(t *testing.T) {
	t.Setenv("RESOLVED_SERVER_ADDRESS", "")
	t.Setenv("RESOLVED_DATABASE_DRIVER", "")
	t.Setenv("RESOLVED_DATABASE_DSN", "")
	t.Setenv("RESOLVED_SESSION_TTL", "")

	cfg, err := Load()
	if err != nil {
		t.Fatalf("load default config: %v", err)
	}
	if cfg.Address != defaultAddress {
		t.Fatalf("address = %q, want %q", cfg.Address, defaultAddress)
	}
	if cfg.Database.Driver != "sqlite" {
		t.Fatalf("driver = %q, want sqlite", cfg.Database.Driver)
	}
	if cfg.Database.DSN != defaultSQLiteDSN {
		t.Fatalf("DSN = %q, want default SQLite DSN", cfg.Database.DSN)
	}
	if cfg.SessionTTL != 24*time.Hour {
		t.Fatalf("session TTL = %s, want 24h", cfg.SessionTTL)
	}
}

func TestLoadAcceptsMSSQLAlias(t *testing.T) {
	t.Setenv("RESOLVED_SERVER_ADDRESS", "127.0.0.1:9000")
	t.Setenv("RESOLVED_DATABASE_DRIVER", "mssql")
	t.Setenv("RESOLVED_DATABASE_DSN", "sqlserver://example")
	t.Setenv("RESOLVED_SESSION_TTL", "2h")

	cfg, err := Load()
	if err != nil {
		t.Fatalf("load SQL Server config: %v", err)
	}
	if cfg.Database.Driver != "sqlserver" {
		t.Fatalf("driver = %q, want sqlserver", cfg.Database.Driver)
	}
}

func TestLoadRequiresDSNForRemoteDatabase(t *testing.T) {
	t.Setenv("RESOLVED_DATABASE_DRIVER", "postgres")
	t.Setenv("RESOLVED_DATABASE_DSN", "")

	if _, err := Load(); err == nil {
		t.Fatal("expected a missing PostgreSQL DSN to fail")
	}
}
