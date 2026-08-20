package activitylog

import (
	"context"
	"fmt"
	"testing"
	"time"

	"resolved-server/internal/identity"
	"resolved-server/internal/resourceevents"

	"github.com/google/uuid"
	"gorm.io/driver/sqlite"
	"gorm.io/gorm"
)

func TestRecorderPersistsActorSnapshotAndBeforeAfterDiff(t *testing.T) {
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
	if err := db.AutoMigrate(&identity.User{}, &Entry{}); err != nil {
		t.Fatalf("migrate database: %v", err)
	}
	now := time.Now().UTC()
	actor := identity.User{
		ID: uuid.NewString(), Email: "actor@example.test", DisplayName: "Audit Actor",
		PasswordHash: "unused", Active: true, CreatedAt: now, UpdatedAt: now,
	}
	if err := db.Create(&actor).Error; err != nil {
		t.Fatalf("create actor: %v", err)
	}

	recorder := NewRecorder(NewRepository(db), nil)
	resourceevents.Emit(recorder, resourceevents.Change{
		Resource: resourceevents.ResourceWorkspace, Action: resourceevents.ActionUpdated,
		ResourceID: uuid.NewString(), WorkspaceID: uuid.NewString(),
		ActorUserID: actor.ID, TargetName: "Production",
		Diffs: []resourceevents.Diff{{Field: "name", From: "Old", To: "Production"}},
	})

	entries, err := NewRepository(db).ListWorkspace(
		context.Background(),
		func() string {
			var entry Entry
			if err := db.First(&entry).Error; err != nil {
				t.Fatalf("load recorded entry: %v", err)
			}
			return entry.WorkspaceID
		}(),
		true,
		nil,
		pageQuery{Limit: DefaultPageLimit},
	)
	if err != nil {
		t.Fatalf("list recorded entries: %v", err)
	}
	views, err := viewEntries(entries.Entries)
	if err != nil {
		t.Fatalf("view recorded entries: %v", err)
	}
	if len(views) != 1 {
		t.Fatalf("entries = %d, want 1", len(views))
	}
	entry := views[0]
	if entry.ActorDisplayName != actor.DisplayName || entry.ActorEmail != actor.Email {
		t.Fatalf("actor snapshot = %+v", entry)
	}
	if len(entry.Diffs) != 1 || entry.Diffs[0].Field != "name" ||
		entry.Diffs[0].From != "Old" || entry.Diffs[0].To != "Production" {
		t.Fatalf("recorded diff = %+v", entry.Diffs)
	}
}
