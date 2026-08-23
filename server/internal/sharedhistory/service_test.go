package sharedhistory

import (
	"fmt"
	"testing"
	"time"

	"resolved-server/internal/identity"
	"resolved-server/internal/resourceevents"
	"resolved-server/internal/workspaces"

	"github.com/google/uuid"
	"gorm.io/driver/sqlite"
	"gorm.io/gorm"
)

type capturedHistoryEvents struct {
	changes []resourceevents.Change
}

func (capture *capturedHistoryEvents) FireEvent(name string, data any) {
	if name == resourceevents.EventName {
		capture.changes = append(capture.changes, data.(resourceevents.Change))
	}
}

func TestServiceEmitsRealtimeChangesToResolvedViewers(t *testing.T) {
	dsn := fmt.Sprintf("file:%s?mode=memory&cache=shared&_foreign_keys=on", uuid.NewString())
	db, err := gorm.Open(sqlite.Open(dsn), &gorm.Config{})
	if err != nil {
		t.Fatalf("open database: %v", err)
	}
	sqlDatabase, err := db.DB()
	if err != nil {
		t.Fatalf("access database: %v", err)
	}
	t.Cleanup(func() { _ = sqlDatabase.Close() })
	if err := db.AutoMigrate(
		&identity.Permission{},
		&identity.Role{},
		&identity.User{},
		&workspaces.Workspace{},
		&workspaces.Collection{},
		&workspaces.WorkspaceUser{},
		&workspaces.CollectionUser{},
		&Entry{},
	); err != nil {
		t.Fatalf("migrate database: %v", err)
	}

	now := time.Now().UTC()
	user := identity.User{
		ID: uuid.NewString(), Email: "author@example.test", DisplayName: "History Author",
		PasswordHash: "unused", Active: true, CreatedAt: now, UpdatedAt: now,
	}
	workspace := workspaces.Workspace{
		ID: uuid.NewString(), Name: "Realtime History", CreatedAt: now, UpdatedAt: now,
	}
	if err := db.Create(&user).Error; err != nil {
		t.Fatalf("create user: %v", err)
	}
	if err := db.Create(&workspace).Error; err != nil {
		t.Fatalf("create workspace: %v", err)
	}

	capture := &capturedHistoryEvents{}
	service := NewService(
		NewRepository(db),
		workspaces.NewService(workspaces.NewRepository(db), nil),
		WithEvents(capture),
	)
	actor := workspaces.Actor{UserID: user.ID, Owner: true}
	if _, err := service.Create(t.Context(), actor, workspace.ID, CreateInput{
		ClientEntryID: "client-entry-1",
		CreatedAt:     now,
		Request: RequestView{
			Method: "GET", URL: "https://example.test", BodyMode: "none",
		},
	}); err != nil {
		t.Fatalf("create shared history: %v", err)
	}
	if err := service.DeleteOwn(t.Context(), actor, workspace.ID); err != nil {
		t.Fatalf("delete shared history: %v", err)
	}

	if len(capture.changes) != 2 {
		t.Fatalf("captured changes = %d, want 2", len(capture.changes))
	}
	for index, action := range []resourceevents.Action{
		resourceevents.ActionUpdated,
		resourceevents.ActionDeleted,
	} {
		change := capture.changes[index]
		if change.Resource != resourceevents.ResourceSharedHistory ||
			change.Action != action ||
			change.ResourceID != user.ID ||
			change.WorkspaceID != workspace.ID {
			t.Fatalf("change %d = %+v", index, change)
		}
		if len(change.Audience.UserIDs) != 1 || change.Audience.UserIDs[0] != user.ID {
			t.Fatalf("change %d audience = %+v, want only history owner", index, change.Audience)
		}
	}
}

func TestRedactSensitiveHeadersCoversCredentialCarryingNames(t *testing.T) {
	input := []Header{
		{Name: "Authorization", Value: "Bearer abc"},
		{Name: "Cookie", Value: "session=secret"},
		{Name: "X-Api-Key", Value: "key-123"},
		{Name: "X-Request-Id", Value: "req-1"},
		{Name: "Content-Type", Value: "application/json"},
		{Name: "X-Auth-Token", Value: "tok"},
		{Name: "X-Amz-Security-Token", Value: "sts"},
	}
	redacted := redactSensitiveHeaders(input)
	for _, header := range redacted {
		if isSensitiveHeader(header.Name) && header.Value != redactedHeaderValue {
			t.Fatalf("sensitive header %q was not redacted: %q", header.Name, header.Value)
		}
		if !isSensitiveHeader(header.Name) && header.Value == redactedHeaderValue {
			t.Fatalf("non-sensitive header %q was redacted", header.Name)
		}
	}
	// Idempotent: re-redacting an already-redacted entry keeps the marker.
	again := redactSensitiveHeaders(redacted)
	for _, header := range again {
		if isSensitiveHeader(header.Name) && header.Value != redactedHeaderValue {
			t.Fatalf("second redaction pass missed %q", header.Name)
		}
	}
}
