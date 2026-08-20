package config

import (
	"testing"
	"time"
)

func TestLoadDefaultsToLocalSQLite(t *testing.T) {
	setDataEncryptionKey(t)
	t.Setenv("RESOLVED_SERVER_ADDRESS", "")
	t.Setenv("RESOLVED_DATABASE_DRIVER", "")
	t.Setenv("RESOLVED_DATABASE_DSN", "")
	t.Setenv("RESOLVED_SESSION_TTL", "")
	t.Setenv("RESOLVED_ENCRYPTION_SECRET", "test deployment encryption secret with enough bytes")

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
	setDataEncryptionKey(t)
	t.Setenv("RESOLVED_SERVER_ADDRESS", "127.0.0.1:9000")
	t.Setenv("RESOLVED_DATABASE_DRIVER", "mssql")
	t.Setenv("RESOLVED_DATABASE_DSN", "sqlserver://example")
	t.Setenv("RESOLVED_SESSION_TTL", "2h")
	t.Setenv("RESOLVED_ENCRYPTION_SECRET", "test deployment encryption secret with enough bytes")

	cfg, err := Load()
	if err != nil {
		t.Fatalf("load SQL Server config: %v", err)
	}
	if cfg.Database.Driver != "sqlserver" {
		t.Fatalf("driver = %q, want sqlserver", cfg.Database.Driver)
	}
}

func TestLoadRequiresDSNForRemoteDatabase(t *testing.T) {
	setDataEncryptionKey(t)
	t.Setenv("RESOLVED_DATABASE_DRIVER", "postgres")
	t.Setenv("RESOLVED_DATABASE_DSN", "")
	t.Setenv("RESOLVED_ENCRYPTION_SECRET", "test deployment encryption secret with enough bytes")

	if _, err := Load(); err == nil {
		t.Fatal("expected a missing PostgreSQL DSN to fail")
	}
}

func TestLoadRequiresEncryptionSecret(t *testing.T) {
	setDataEncryptionKey(t)
	t.Setenv("RESOLVED_DATABASE_DRIVER", "sqlite")
	t.Setenv("RESOLVED_DATABASE_DSN", ":memory:")
	t.Setenv("RESOLVED_ENCRYPTION_SECRET", "too short")

	if _, err := Load(); err == nil {
		t.Fatal("expected a short encryption secret to fail")
	}
}

func TestLoadRequiresDataEncryptionKey(t *testing.T) {
	t.Setenv("RESOLVED_DATA_KEY_PROVIDER", "static")
	t.Setenv("RESOLVED_DATABASE_DRIVER", "sqlite")
	t.Setenv("RESOLVED_DATABASE_DSN", ":memory:")
	t.Setenv("RESOLVED_ENCRYPTION_SECRET", "test deployment encryption secret with enough bytes")
	t.Setenv("RESOLVED_DATA_ENCRYPTION_KEY", "")

	if _, err := Load(); err == nil {
		t.Fatal("expected a missing data encryption key to fail")
	}
}

func TestLoadAcceptsVaultDataKeyProvider(t *testing.T) {
	t.Setenv("RESOLVED_DATABASE_DRIVER", "sqlite")
	t.Setenv("RESOLVED_DATABASE_DSN", ":memory:")
	t.Setenv("RESOLVED_ENCRYPTION_SECRET", "test deployment encryption secret with enough bytes")
	t.Setenv("RESOLVED_DATA_KEY_PROVIDER", "vault")
	t.Setenv("RESOLVED_VAULT_ADDRESS", "https://vault.example.test")
	t.Setenv("RESOLVED_VAULT_TOKEN", "test-token")
	t.Setenv("RESOLVED_VAULT_TRANSIT_KEY", "resolved-server")

	cfg, err := Load()
	if err != nil {
		t.Fatalf("load Vault config: %v", err)
	}
	if cfg.DataEncryption.Provider != "vault" || cfg.DataEncryption.VaultMount != defaultVaultMount {
		t.Fatalf("data encryption config = %+v", cfg.DataEncryption)
	}
}

func setDataEncryptionKey(t *testing.T) {
	t.Helper()
	t.Setenv("RESOLVED_DATA_KEY_PROVIDER", "static")
	t.Setenv("RESOLVED_DATA_ENCRYPTION_KEY", "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=")
}
