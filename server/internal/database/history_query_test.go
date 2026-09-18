package database_test

import (
	"fmt"
	"testing"
	"time"

	"resolved-server/internal/identity"
	"resolved-server/internal/sharedhistory"
	"resolved-server/internal/workspaces"

	"github.com/google/uuid"
)

func TestEncryptedHistoryFiltersBeforeListingLimit(t *testing.T) {
	db, cipher := encryptedRepositoryTestDatabase(t)
	sqlDB, err := db.DB()
	if err != nil {
		t.Fatal(err)
	}
	// Decryption must not keep list rows open while loading workspace keys.
	sqlDB.SetMaxOpenConns(1)
	now := time.Now().UTC()
	user := identity.User{ID: uuid.NewString(), Email: "history-query@example.test", DisplayName: "History", PasswordHash: "unused", Active: true, CreatedAt: now, UpdatedAt: now}
	workspace := workspaces.Workspace{ID: uuid.NewString(), Name: "History", CreatedAt: now, UpdatedAt: now}
	if err := db.Create(&user).Error; err != nil {
		t.Fatal(err)
	}
	if err := db.Create(&workspace).Error; err != nil {
		t.Fatal(err)
	}
	repository := sharedhistory.NewRepository(db, cipher)
	for i := 0; i < 30; i++ {
		method := "GET"
		if i == 0 {
			method = "PATCH"
		}
		_, err := repository.UpsertEntry(t.Context(), sharedhistory.Entry{
			ID: uuid.NewString(), WorkspaceID: workspace.ID, UserID: user.ID,
			ClientEntryID: fmt.Sprintf("entry-%02d", i), Method: method,
			URL:                "https://private.example.test/Widgets?foo%20bar=value",
			RequestHeadersJSON: []byte(`[{"name":"X-Test","value":"secret"}]`),
			RequestBody:        []byte{}, RequestBodyMode: "raw", RequestBodyLanguage: "json",
			RequestBodyFieldsJSON: []byte("[]"), ResponseHeadersJSON: []byte("[]"),
			CreatedAt: now.Add(time.Duration(i) * time.Second), UpdatedAt: now,
		})
		if err != nil {
			t.Fatal(err)
		}
	}
	var stored sharedhistory.Entry
	if err := db.First(&stored).Error; err != nil {
		t.Fatal(err)
	}
	if len(stored.EncryptedPayload) == 0 || stored.Method != "" || stored.URL != "" {
		t.Fatal("history fields are not encrypted")
	}
	got, err := repository.ListEntries(t.Context(), workspace.ID, user.ID, sharedhistory.ListOptions{
		Method: "patch", Status: "error", Hostname: "PRIVATE", Path: "Widgets",
		HeaderKeys: "x-test", ParamKeys: "foo bar", BodyType: "raw:json",
	})
	if err != nil || len(got) != 1 || got[0].ClientEntryID != "entry-00" {
		t.Fatalf("older encrypted match = %+v, %v", got, err)
	}
	if got[0].EncryptedPayload != nil {
		t.Fatal("listing retained ciphertext alongside plaintext")
	}
	got, err = repository.ListEntries(t.Context(), workspace.ID, user.ID, sharedhistory.ListOptions{Sort: "oldest"})
	if err != nil || len(got) != 20 || got[0].ClientEntryID != "entry-00" || got[19].ClientEntryID != "entry-19" {
		t.Fatalf("oldest encrypted listing = %+v, %v", got, err)
	}
	got, err = repository.ListEntries(t.Context(), uuid.NewString(), user.ID, sharedhistory.ListOptions{Method: "patch"})
	if err != nil || len(got) != 0 {
		t.Fatalf("cross-workspace results = %+v, %v", got, err)
	}
}
