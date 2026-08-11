package main

import (
	"bytes"
	"os"
	"testing"
)

func TestMissingEncryptionSecretIsReportedWithoutPanicking(t *testing.T) {
	t.Setenv("RESOLVED_DATABASE_DRIVER", "sqlite")
	t.Setenv("RESOLVED_DATABASE_DSN", "resolved-test.db")
	t.Setenv("RESOLVED_SERVER_ADDRESS", "127.0.0.1:8787")
	t.Setenv("RESOLVED_SESSION_TTL", "24h")
	t.Setenv("RESOLVED_ENCRYPTION_SECRET", "")

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	err := run([]string{"serve"}, os.Stdin, &stdout, &stderr)
	if err == nil {
		t.Fatal("run returned nil without the required encryption secret")
	}

	var reported bytes.Buffer
	reportRunError(&reported, err)
	const expected = "resolved-server: RESOLVED_ENCRYPTION_SECRET must contain at least 32 bytes\n"
	if reported.String() != expected {
		t.Fatalf("reported error = %q, want %q", reported.String(), expected)
	}
}
