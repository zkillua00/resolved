package executionlimits

import (
	"math"
	"path/filepath"
	"testing"

	"resolved-server/internal/workspaces"

	"gorm.io/driver/sqlite"
	"gorm.io/gorm"
)

func testProvider(t *testing.T) *Provider {
	t.Helper()
	db, err := gorm.Open(sqlite.Open(filepath.Join(t.TempDir(), "limits.db")), &gorm.Config{})
	if err != nil {
		t.Fatal(err)
	}
	sqlDB, _ := db.DB()
	t.Cleanup(func() { _ = sqlDB.Close() })
	if err := db.AutoMigrate(&Record{}, &workspaces.Workspace{}, &workspaces.Collection{}, &workspaces.WorkspaceUser{}, &workspaces.CollectionUser{}); err != nil {
		t.Fatal(err)
	}
	for _, workspace := range []workspaces.Workspace{{ID: "w", Name: "workspace"}, {ID: "other", Name: "other"}} {
		if err := db.Create(&workspace).Error; err != nil {
			t.Fatal(err)
		}
	}
	parent := "root"
	for _, collection := range []workspaces.Collection{
		{ID: "root", WorkspaceID: "w", Name: "root"},
		{ID: "child", WorkspaceID: "w", Name: "child", ParentCollectionID: &parent},
		{ID: "sibling", WorkspaceID: "w", Name: "sibling"},
		{ID: "unrelated", WorkspaceID: "other", Name: "unrelated"},
	} {
		if err := db.Create(&collection).Error; err != nil {
			t.Fatal(err)
		}
	}
	return NewProvider(db)
}

func TestHierarchyLiveReplacementAndProvenance(t *testing.T) {
	p := testProvider(t)
	ctx := t.Context()
	const key = "http.response_bytes"
	scopes := []Scope{{}, {WorkspaceID: "w"}, {WorkspaceID: "w", CollectionID: "root"}, {WorkspaceID: "w", CollectionID: "child"}}
	bounds := []Bound{{Value: 1}, {Value: 1000000000}, {Unlimited: true}, {Value: 0}}
	for i, scope := range scopes {
		if _, err := p.Replace(ctx, scope, Limits{key: bounds[i]}); err != nil {
			t.Fatal(err)
		}
		snapshot, err := p.Resolve(ctx, scopes[3])
		if err != nil {
			t.Fatal(err)
		}
		if snapshot.Effective[key] != bounds[i] {
			t.Fatalf("layer %d: %+v", i, snapshot)
		}
		if snapshot.Sources[key].CollectionID != scope.CollectionID || snapshot.Sources[key].WorkspaceID != scope.WorkspaceID {
			t.Fatalf("wrong source: %+v", snapshot.Sources[key])
		}
	}
	for i := 3; i >= 0; i-- {
		if _, err := p.Replace(ctx, scopes[i], Limits{}); err != nil {
			t.Fatal(err)
		}
		snapshot, err := NewProvider(p.db).Resolve(ctx, scopes[3])
		if err != nil {
			t.Fatal(err)
		}
		expected := Bound{Value: 67108864}
		if i > 0 {
			expected = bounds[i-1]
		}
		if snapshot.Effective[key] != expected || len(snapshot.Overrides) != 0 {
			t.Fatalf("reset %d: %+v", i, snapshot)
		}
	}
	snapshot, err := p.Resolve(ctx, Scope{})
	if err != nil || len(snapshot.Definitions) != len(definitions) || snapshot.Sources[key].Kind != "default" {
		t.Fatalf("%+v %v", snapshot, err)
	}
}

func TestInvalidLimitsDoNotReplaceLayer(t *testing.T) {
	p := testProvider(t)
	valid := Limits{"http.redirects": {Value: 4}}
	if _, err := p.Replace(t.Context(), Scope{}, valid); err != nil {
		t.Fatal(err)
	}
	for _, invalid := range []Limits{
		{"unknown": {}},
		{"http.redirects": {Value: -1}},
		{"http.redirects": {Unlimited: true, Value: 1}},
		{"script.timeout_ms": {Value: math.MaxInt64}},
	} {
		if _, err := p.Replace(t.Context(), Scope{}, invalid); err == nil {
			t.Fatalf("accepted %+v", invalid)
		}
		snapshot, err := p.Resolve(t.Context(), Scope{})
		if err != nil || snapshot.Overrides["http.redirects"].Value != 4 {
			t.Fatalf("changed layer: %+v %v", snapshot, err)
		}
	}
	if err := Validate(Limits{"http.response_bytes": {Value: math.MaxInt64}}); err != nil {
		t.Fatalf("arbitrary ceiling: %v", err)
	}
}

func TestScopeValidationAndPersistenceErrors(t *testing.T) {
	p := testProvider(t)
	for _, scope := range []Scope{{CollectionID: "child"}, {WorkspaceID: "missing"}, {WorkspaceID: "w", CollectionID: "unrelated"}, {WorkspaceID: "w", CollectionID: "missing"}} {
		if _, err := p.Resolve(t.Context(), scope); err == nil {
			t.Fatalf("accepted %+v", scope)
		}
	}
	if err := p.db.Model(&workspaces.Collection{}).Where("id = ?", "root").Update("parent_collection_id", "child").Error; err != nil {
		t.Fatal(err)
	}
	if _, err := p.Resolve(t.Context(), Scope{WorkspaceID: "w", CollectionID: "child"}); err == nil {
		t.Fatal("cycle accepted")
	}
	if err := p.db.Migrator().DropTable(&Record{}); err != nil {
		t.Fatal(err)
	}
	if _, err := p.Resolve(t.Context(), Scope{}); err == nil {
		t.Fatal("DB error silently defaulted")
	}
}

func TestGrantScopeUsesActualAncestors(t *testing.T) {
	p := testProvider(t)
	if err := p.db.Create(&workspaces.CollectionUser{CollectionID: "root", UserID: "member"}).Error; err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		scope   Scope
		allowed bool
	}{
		{Scope{WorkspaceID: "w", CollectionID: "root"}, true},
		{Scope{WorkspaceID: "w", CollectionID: "child"}, true},
		{Scope{WorkspaceID: "w", CollectionID: "sibling"}, false},
		{Scope{WorkspaceID: "w"}, false},
		{Scope{WorkspaceID: "other", CollectionID: "unrelated"}, false},
	} {
		layers, err := ancestry(p.db, tc.scope)
		if err != nil {
			t.Fatal(err)
		}
		err = authorize(p.db, layers, "member", false)
		if (err == nil) != tc.allowed {
			t.Fatalf("%+v: %v", tc.scope, err)
		}
	}
	if err := p.db.Create(&workspaces.WorkspaceUser{WorkspaceID: "w", UserID: "member"}).Error; err != nil {
		t.Fatal(err)
	}
	layers, err := ancestry(p.db, Scope{WorkspaceID: "w"})
	if err != nil {
		t.Fatal(err)
	}
	if err := authorize(p.db, layers, "member", false); err != nil {
		t.Fatal(err)
	}
}
