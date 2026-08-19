package sharedhistory

import (
	"fmt"
	"slices"
	"testing"
	"time"

	"resolved-server/internal/identity"
	"resolved-server/internal/workspaces"

	"github.com/google/uuid"
	"gorm.io/driver/sqlite"
	"gorm.io/gorm"
)

func TestRepositoryRetainsOneHundredEntriesAndListsLatestTwenty(t *testing.T) {
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
	if err := db.AutoMigrate(&identity.User{}, &workspaces.Workspace{}, &Entry{}); err != nil {
		t.Fatalf("migrate database: %v", err)
	}

	user := identity.User{
		ID: uuid.NewString(), Email: "history@example.test", DisplayName: "History User",
		PasswordHash: "unused", Active: true, CreatedAt: time.Now().UTC(), UpdatedAt: time.Now().UTC(),
	}
	workspace := workspaces.Workspace{
		ID: uuid.NewString(), Name: "History Workspace", CreatedAt: time.Now().UTC(), UpdatedAt: time.Now().UTC(),
	}
	if err := db.Create(&user).Error; err != nil {
		t.Fatalf("create user: %v", err)
	}
	if err := db.Create(&workspace).Error; err != nil {
		t.Fatalf("create workspace: %v", err)
	}

	repository := NewRepository(db)
	baseTime := time.Now().UTC().Add(-time.Hour)
	var newestID string
	for index := 0; index < 105; index++ {
		stored, err := repository.UpsertEntry(t.Context(), Entry{
			ID: uuid.NewString(), WorkspaceID: workspace.ID, UserID: user.ID,
			ClientEntryID: fmt.Sprintf("entry-%03d", index), Method: "GET",
			URL: "https://example.test", RequestHeadersJSON: []byte("[]"), RequestBody: []byte{}, RequestBodyMode: "none",
			RequestBodyFieldsJSON: []byte("[]"), ResponseHeadersJSON: []byte("[]"),
			CreatedAt: baseTime.Add(time.Duration(index) * time.Second), UpdatedAt: time.Now().UTC(),
		})
		if err != nil {
			t.Fatalf("save entry %d: %v", index, err)
		}
		if index == 104 {
			newestID = stored.ID
		}
	}
	updated, err := repository.UpsertEntry(t.Context(), Entry{
		ID: uuid.NewString(), WorkspaceID: workspace.ID, UserID: user.ID,
		ClientEntryID: "entry-104", Method: "PATCH", URL: "https://example.test",
		RequestHeadersJSON: []byte("[]"), RequestBody: []byte{}, RequestBodyMode: "none",
		RequestBodyFieldsJSON: []byte("[]"), ResponseHeadersJSON: []byte("[]"),
		CreatedAt: baseTime.Add(104 * time.Second), UpdatedAt: time.Now().UTC(),
	})
	if err != nil {
		t.Fatalf("retry newest entry: %v", err)
	}
	if updated.ID != newestID || updated.Method != "PATCH" {
		t.Fatalf("retried entry = %+v, want ID %s and updated method", updated, newestID)
	}

	var retained int64
	if err := db.Model(&Entry{}).
		Where("workspace_id = ? AND user_id = ?", workspace.ID, user.ID).
		Count(&retained).Error; err != nil {
		t.Fatalf("count retained entries: %v", err)
	}
	if retained != MaxEntriesPerProfileWorkspace {
		t.Fatalf("retained entries = %d, want %d", retained, MaxEntriesPerProfileWorkspace)
	}

	listed, err := repository.ListEntries(t.Context(), workspace.ID, user.ID)
	if err != nil {
		t.Fatalf("list entries: %v", err)
	}
	if len(listed) != MaxListedEntries {
		t.Fatalf("listed entries = %d, want %d", len(listed), MaxListedEntries)
	}
	if listed[0].ClientEntryID != "entry-104" || listed[len(listed)-1].ClientEntryID != "entry-085" {
		t.Fatalf("listed range = %s through %s", listed[0].ClientEntryID, listed[len(listed)-1].ClientEntryID)
	}

	if err := db.Delete(&workspace).Error; err != nil {
		t.Fatalf("delete workspace: %v", err)
	}
	if err := db.Model(&Entry{}).Count(&retained).Error; err != nil {
		t.Fatalf("count entries after workspace deletion: %v", err)
	}
	if retained != 0 {
		t.Fatalf("entries after workspace deletion = %d, want 0", retained)
	}
}

func TestRepositoryScopesRealtimeViewersByPermissionAndWorkspaceAccess(t *testing.T) {
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
	user := func(id string, active bool) identity.User {
		return identity.User{
			ID: id, Email: id + "@example.test", DisplayName: id,
			PasswordHash: "unused", Active: active, CreatedAt: now, UpdatedAt: now,
		}
	}
	author := user(uuid.NewString(), true)
	owner := user(uuid.NewString(), true)
	viewer := user(uuid.NewString(), true)
	withoutPermission := user(uuid.NewString(), true)
	withoutAccess := user(uuid.NewString(), true)
	inactive := user(uuid.NewString(), false)
	for _, account := range []identity.User{
		author, owner, viewer, withoutPermission, withoutAccess, inactive,
	} {
		if err := db.Create(&account).Error; err != nil {
			t.Fatalf("create user %s: %v", account.ID, err)
		}
	}
	if err := db.Model(&identity.User{}).
		Where("id = ?", inactive.ID).
		Update("active", false).Error; err != nil {
		t.Fatalf("deactivate realtime viewer: %v", err)
	}
	permission := identity.Permission{
		Key: identity.PermissionHistoryReadOthers, Description: "View shared history",
	}
	readerRole := identity.Role{
		ID: uuid.NewString(), Name: "History reader", NormalizedName: "history reader",
		CreatedAt: now, UpdatedAt: now,
	}
	ownerRole := identity.Role{
		ID: identity.OwnerRoleID, Name: "Owner", NormalizedName: "owner", System: true,
		CreatedAt: now, UpdatedAt: now,
	}
	if err := db.Create(&permission).Error; err != nil {
		t.Fatalf("create permission: %v", err)
	}
	if err := db.Create(&readerRole).Error; err != nil {
		t.Fatalf("create reader role: %v", err)
	}
	if err := db.Create(&ownerRole).Error; err != nil {
		t.Fatalf("create owner role: %v", err)
	}
	if err := db.Exec(
		"INSERT INTO role_permissions (role_id, permission_key) VALUES (?, ?)",
		readerRole.ID, permission.Key,
	).Error; err != nil {
		t.Fatalf("grant reader permission: %v", err)
	}
	for _, userID := range []string{viewer.ID, withoutAccess.ID, inactive.ID} {
		if err := db.Exec(
			"INSERT INTO user_roles (user_id, role_id) VALUES (?, ?)",
			userID, readerRole.ID,
		).Error; err != nil {
			t.Fatalf("assign reader role to %s: %v", userID, err)
		}
	}
	if err := db.Exec(
		"INSERT INTO user_roles (user_id, role_id) VALUES (?, ?)",
		owner.ID, ownerRole.ID,
	).Error; err != nil {
		t.Fatalf("assign owner role: %v", err)
	}

	workspace := workspaces.Workspace{
		ID: uuid.NewString(), Name: "Realtime history", CreatedAt: now, UpdatedAt: now,
	}
	collection := workspaces.Collection{
		ID: uuid.NewString(), WorkspaceID: workspace.ID, Name: "API",
		CreatedAt: now, UpdatedAt: now,
	}
	if err := db.Create(&workspace).Error; err != nil {
		t.Fatalf("create workspace: %v", err)
	}
	if err := db.Create(&collection).Error; err != nil {
		t.Fatalf("create collection: %v", err)
	}
	for _, userID := range []string{author.ID, withoutPermission.ID, inactive.ID} {
		if err := db.Create(&workspaces.WorkspaceUser{
			WorkspaceID: workspace.ID, UserID: userID, CreatedAt: now,
		}).Error; err != nil {
			t.Fatalf("grant workspace access to %s: %v", userID, err)
		}
	}
	if err := db.Create(&workspaces.CollectionUser{
		CollectionID: collection.ID, UserID: viewer.ID, CreatedAt: now,
	}).Error; err != nil {
		t.Fatalf("grant collection access to viewer: %v", err)
	}

	got, err := NewRepository(db).ListRealtimeViewerUserIDs(
		t.Context(), workspace.ID, author.ID,
	)
	if err != nil {
		t.Fatalf("list realtime viewers: %v", err)
	}
	want := []string{author.ID, owner.ID, viewer.ID}
	slices.Sort(want)
	if !slices.Equal(got, want) {
		t.Fatalf("realtime viewers = %v, want %v", got, want)
	}
}
