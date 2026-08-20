package bootstrap

import (
	"bytes"
	"encoding/base64"
	"io"
	"os"
	"path/filepath"
	"testing"
	"time"

	"resolved-server/internal/config"
	"resolved-server/internal/database"
	"resolved-server/internal/identity"
	"resolved-server/internal/sharedhistory"
	"resolved-server/internal/users"
	"resolved-server/internal/workspaces"

	"github.com/google/uuid"
	"gorm.io/driver/sqlite"
	"gorm.io/gorm"
)

func TestBootstrapOwnerPersistsEncryptedIdentityAndInitialWorkspace(t *testing.T) {
	path := filepath.Join(t.TempDir(), "resolved.db")
	cfg := config.Config{
		Address: "127.0.0.1:0",
		Database: config.Database{
			Driver: "sqlite",
			DSN:    path + "?_busy_timeout=5000&_journal_mode=WAL&_foreign_keys=on",
		},
		SessionTTL:       60_000_000_000,
		EncryptionSecret: "test environment encryption secret with enough bytes",
		DataEncryption: config.DataEncryption{
			Provider: "static", KeyID: "test-root-v1",
			EncodedKey: base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0x71}, 32)),
		},
	}
	application, err := New(cfg, io.Discard)
	if err != nil {
		t.Fatalf("create application: %v", err)
	}
	owner, err := application.Users.BootstrapOwner(t.Context(), users.CreateInput{
		Email: "owner", DisplayName: "Deployment Owner", Password: "a sufficiently long password",
	})
	if err != nil {
		_ = application.Close()
		t.Fatalf("bootstrap owner: %v", err)
	}
	if owner.Email != "owner" || owner.DisplayName != "Deployment Owner" {
		_ = application.Close()
		t.Fatalf("owner round trip = %+v", owner)
	}
	if err := application.Close(); err != nil {
		t.Fatalf("close application: %v", err)
	}

	db, err := gorm.Open(sqlite.Open(path), &gorm.Config{})
	if err != nil {
		t.Fatalf("reopen database: %v", err)
	}
	sqlDB, err := db.DB()
	if err != nil {
		t.Fatalf("access reopened database: %v", err)
	}
	defer sqlDB.Close()
	var rawOwner identity.User
	if err := db.First(&rawOwner, "id = ?", owner.ID).Error; err != nil {
		t.Fatalf("load raw owner: %v", err)
	}
	if rawOwner.Email != "" || rawOwner.DisplayName != "" || len(rawOwner.EncryptedProfile) == 0 {
		t.Fatalf("raw owner contains profile plaintext: %+v", rawOwner)
	}
	var rawWorkspace workspaces.Workspace
	if err := db.First(&rawWorkspace).Error; err != nil {
		t.Fatalf("load raw initial workspace: %v", err)
	}
	if rawWorkspace.Name != "" || len(rawWorkspace.EncryptedName) == 0 {
		t.Fatalf("raw initial workspace contains name plaintext: %+v", rawWorkspace)
	}
}

func TestStartupEncryptsLegacyRowsAndRebuildsSQLiteWithoutPlaintext(t *testing.T) {
	path := filepath.Join(t.TempDir(), "legacy.db")
	dsn := path + "?_busy_timeout=5000&_journal_mode=WAL&_foreign_keys=on"
	db, err := database.Open(config.Database{Driver: "sqlite", DSN: dsn})
	if err != nil {
		t.Fatalf("open legacy database: %v", err)
	}
	if err := database.MigrateAndSeed(db); err != nil {
		t.Fatalf("migrate legacy database: %v", err)
	}
	now := time.Now().UTC()
	user := identity.User{
		ID: uuid.NewString(), Email: "legacy-owner", DisplayName: "LEGACY_PROFILE_SENTINEL",
		PasswordHash: "unused", Active: true, CreatedAt: now, UpdatedAt: now,
	}
	workspace := workspaces.Workspace{
		ID: uuid.NewString(), Name: "LEGACY_WORKSPACE_SENTINEL", CreatedAt: now, UpdatedAt: now,
	}
	collection := workspaces.Collection{
		ID: uuid.NewString(), WorkspaceID: workspace.ID, Name: "Legacy collection", CreatedAt: now, UpdatedAt: now,
	}
	request := workspaces.SavedRequest{
		ID: uuid.NewString(), CollectionID: collection.ID, Name: "Legacy request",
		Definition: `{"secret":"LEGACY_REQUEST_SENTINEL"}`, CreatedAt: now, UpdatedAt: now,
	}
	history := sharedhistory.Entry{
		ID: uuid.NewString(), WorkspaceID: workspace.ID, UserID: user.ID,
		ClientEntryID: "LEGACY_HISTORY_ID_SENTINEL", Method: "POST",
		URL:                `https://example.test/LEGACY_HISTORY_URL_SENTINEL`,
		RequestHeadersJSON: []byte("[]"), RequestBody: []byte("LEGACY_HISTORY_BODY_SENTINEL"),
		RequestBodyMode: "raw", RequestBodyFieldsJSON: []byte("[]"),
		ResponseHeadersJSON: []byte("[]"), CreatedAt: now, UpdatedAt: now,
	}
	for _, record := range []any{&user, &workspace, &collection, &request, &history} {
		if err := db.Create(record).Error; err != nil {
			t.Fatalf("create legacy fixture %T: %v", record, err)
		}
	}
	sqlDB, err := db.DB()
	if err != nil {
		t.Fatalf("access legacy database: %v", err)
	}
	if err := sqlDB.Close(); err != nil {
		t.Fatalf("close legacy database: %v", err)
	}

	cfg := config.Config{
		Address: "127.0.0.1:0", Database: config.Database{Driver: "sqlite", DSN: dsn},
		SessionTTL: time.Hour, EncryptionSecret: "test environment encryption secret with enough bytes",
		DataEncryption: config.DataEncryption{
			Provider: "static", KeyID: "test-root-v1",
			EncodedKey: base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0x72}, 32)),
		},
	}
	application, err := New(cfg, io.Discard)
	if err != nil {
		t.Fatalf("start application on legacy database: %v", err)
	}
	if err := application.Close(); err != nil {
		t.Fatalf("close migrated application: %v", err)
	}
	for _, file := range []string{path, path + "-wal"} {
		contents, err := os.ReadFile(file)
		if err != nil {
			if os.IsNotExist(err) {
				continue
			}
			t.Fatalf("read migrated SQLite file %s: %v", file, err)
		}
		for _, sentinel := range []string{
			"LEGACY_PROFILE_SENTINEL", "LEGACY_WORKSPACE_SENTINEL", "LEGACY_REQUEST_SENTINEL",
			"LEGACY_HISTORY_ID_SENTINEL", "LEGACY_HISTORY_URL_SENTINEL", "LEGACY_HISTORY_BODY_SENTINEL",
		} {
			if bytes.Contains(contents, []byte(sentinel)) {
				t.Fatalf("migrated SQLite file %s still contains %s", file, sentinel)
			}
		}
	}
}
