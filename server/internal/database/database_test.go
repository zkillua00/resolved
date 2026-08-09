package database

import (
	"testing"

	"resolved-server/internal/config"
)

func TestDialectorSupportsConfiguredDatabases(t *testing.T) {
	tests := []struct {
		driver string
		dsn    string
		name   string
	}{
		{driver: "sqlite", dsn: ":memory:", name: "sqlite"},
		{driver: "postgres", dsn: "host=localhost", name: "postgres"},
		{driver: "mysql", dsn: "user:pass@tcp(localhost)/db", name: "mysql"},
		{driver: "sqlserver", dsn: "sqlserver://localhost", name: "sqlserver"},
	}

	for _, test := range tests {
		t.Run(test.driver, func(t *testing.T) {
			dialector, err := Dialector(config.Database{Driver: test.driver, DSN: test.dsn})
			if err != nil {
				t.Fatalf("create dialector: %v", err)
			}
			if dialector.Name() != test.name {
				t.Fatalf("dialector name = %q, want %q", dialector.Name(), test.name)
			}
		})
	}
}

func TestSQLitePathSkipsMemoryDSNs(t *testing.T) {
	for _, dsn := range []string{":memory:", "file::memory:?cache=shared", "file:test?mode=memory&cache=shared"} {
		if path := sqlitePath(dsn); path != "" {
			t.Fatalf("sqlitePath(%q) = %q, want empty", dsn, path)
		}
	}
}
