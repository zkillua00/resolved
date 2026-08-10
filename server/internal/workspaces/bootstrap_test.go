package workspaces_test

import (
	"fmt"
	"testing"

	"resolved-server/internal/config"
	"resolved-server/internal/database"
	"resolved-server/internal/identity"
	"resolved-server/internal/workspaces"

	"github.com/google/uuid"
)

func TestEnsureInitialWorkspaceBackfillsAnExistingOwnerOnce(t *testing.T) {
	dsn := fmt.Sprintf("file:%s?mode=memory&cache=shared&_foreign_keys=on", uuid.NewString())
	db, err := database.Open(config.Database{Driver: "sqlite", DSN: dsn})
	if err != nil {
		t.Fatalf("open database: %v", err)
	}
	sqlDatabase, err := db.DB()
	if err != nil {
		t.Fatalf("access database: %v", err)
	}
	defer func() { _ = sqlDatabase.Close() }()
	if err := database.MigrateAndSeed(db); err != nil {
		t.Fatalf("migrate database: %v", err)
	}

	owner, err := identity.NewRepository(db).CreateFirstOwner(t.Context(), identity.User{
		ID:           uuid.NewString(),
		Email:        "owner",
		DisplayName:  "Owner",
		PasswordHash: "unused-test-hash",
		Active:       true,
	})
	if err != nil {
		t.Fatalf("create owner without workspace setup: %v", err)
	}

	if err := workspaces.EnsureInitialWorkspace(t.Context(), db); err != nil {
		t.Fatalf("ensure initial workspace: %v", err)
	}
	if err := workspaces.EnsureInitialWorkspace(t.Context(), db); err != nil {
		t.Fatalf("ensure initial workspace again: %v", err)
	}

	stored, err := workspaces.NewRepository(db).ListWorkspaces(t.Context())
	if err != nil {
		t.Fatalf("list workspaces: %v", err)
	}
	if len(stored) != 1 || stored[0].Name != workspaces.DefaultWorkspaceName {
		t.Fatalf("workspaces = %+v, want one %q workspace", stored, workspaces.DefaultWorkspaceName)
	}
	if len(stored[0].UserIDs) != 1 || stored[0].UserIDs[0] != owner.ID {
		t.Fatalf("workspace users = %v, want [%s]", stored[0].UserIDs, owner.ID)
	}
}
