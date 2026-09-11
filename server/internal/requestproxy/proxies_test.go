package requestproxy

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
	"testing"
	"time"

	"resolved-server/internal/identity"
	"resolved-server/internal/problem"
	"resolved-server/internal/security"
	"resolved-server/internal/workspaces"

	"github.com/google/uuid"
	"gorm.io/driver/sqlite"
	"gorm.io/gorm"
)

func newProxyTestRepositories(t *testing.T) (*gorm.DB, *ProxyRepository, *SettingsRepository) {
	t.Helper()
	db, err := gorm.Open(sqlite.Open("file:"+uuid.NewString()+"?mode=memory&cache=shared"), &gorm.Config{})
	if err != nil {
		t.Fatalf("open database: %v", err)
	}
	sqlDB, err := db.DB()
	if err != nil {
		t.Fatalf("access database: %v", err)
	}
	t.Cleanup(func() { _ = sqlDB.Close() })
	if err := db.AutoMigrate(
		&security.DataEncryptionKey{}, &identity.User{}, &identity.Role{},
		&workspaces.Workspace{}, &workspaces.Collection{}, &workspaces.SavedRequest{},
		&SettingsRecord{}, &HostnameOverrideRecord{},
		&ProxyRecord{}, &ProxyAssignmentRecord{}, &ProxyExclusionRecord{},
	); err != nil {
		t.Fatalf("migrate database: %v", err)
	}
	provider, err := security.NewStaticKeyProvider(
		"proxy-test-v1",
		base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0x21}, security.DataKeyLength)),
	)
	if err != nil {
		t.Fatalf("create key provider: %v", err)
	}
	dataCipher, err := security.NewDataCipher(provider)
	if err != nil {
		t.Fatalf("create data cipher: %v", err)
	}
	t.Cleanup(dataCipher.Close)
	return db, NewProxyRepository(db, dataCipher), NewSettingsRepository(db, dataCipher)
}

func createProxyScopeFixtures(t *testing.T, db *gorm.DB) (workspaceID, collectionID, requestID string) {
	t.Helper()
	now := time.Now().UTC()
	workspace := workspaces.Workspace{ID: uuid.NewString(), Name: "Workspace", CreatedAt: now, UpdatedAt: now}
	collection := workspaces.Collection{
		ID: uuid.NewString(), WorkspaceID: workspace.ID, Name: "Collection", CreatedAt: now, UpdatedAt: now,
	}
	saved := workspaces.SavedRequest{
		ID: uuid.NewString(), CollectionID: collection.ID, Name: "Request",
		Definition: "{}", CreatedAt: now, UpdatedAt: now,
	}
	if err := db.Create(&workspace).Error; err != nil {
		t.Fatalf("create workspace: %v", err)
	}
	if err := db.Create(&collection).Error; err != nil {
		t.Fatalf("create collection: %v", err)
	}
	if err := db.Create(&saved).Error; err != nil {
		t.Fatalf("create saved request: %v", err)
	}
	return workspace.ID, collection.ID, saved.ID
}

func TestProxyRepositoryCrudAssignmentsAndExclusions(t *testing.T) {
	db, proxies, _ := newProxyTestRepositories(t)
	workspaceID, collectionID, requestID := createProxyScopeFixtures(t, db)
	now := time.Now().UTC()
	user := identity.User{
		ID: uuid.NewString(), Email: "user", DisplayName: "User", PasswordHash: "unused",
		Active: true, CreatedAt: now, UpdatedAt: now,
	}
	role := identity.Role{
		ID: uuid.NewString(), Name: "Role", NormalizedName: "role", CreatedAt: now, UpdatedAt: now,
	}
	if err := db.Create(&user).Error; err != nil {
		t.Fatalf("create user: %v", err)
	}
	if err := db.Create(&role).Error; err != nil {
		t.Fatalf("create role: %v", err)
	}

	proxy, err := proxies.Create(t.Context(), &user.ID, "  Internal routing  ", []HostnameOverride{
		{Hostname: "B.Demo.", Target: "https://Gateway.Internal"},
		{Hostname: "a.demo", Target: "10.0.0.8"},
	})
	if err != nil {
		t.Fatalf("create proxy: %v", err)
	}
	if proxy.Name != "Internal routing" || len(proxy.Rules) != 2 || proxy.Rules[0].Hostname != "a.demo" {
		t.Fatalf("created proxy = %+v", proxy)
	}

	updatedName := "Renamed routing"
	updatedRules := []HostnameOverride{{Hostname: "c.demo", Target: "127.0.0.1"}}
	updated, err := proxies.Update(t.Context(), proxy.ID, &updatedName, &updatedRules)
	if err != nil {
		t.Fatalf("update proxy: %v", err)
	}
	if updated.Name != updatedName || len(updated.Rules) != 1 || updated.Rules[0].Hostname != "c.demo" {
		t.Fatalf("updated proxy = %+v", updated)
	}

	assigned, err := proxies.ReplaceAssignments(t.Context(), proxy.ID, []ProxyAssignment{
		{ScopeKind: ProxyScopeServer},
		{ScopeKind: ProxyScopeWorkspace, ScopeID: workspaceID},
		{ScopeKind: ProxyScopeCollection, ScopeID: collectionID},
		{ScopeKind: ProxyScopeRequest, ScopeID: requestID},
	})
	if err != nil {
		t.Fatalf("replace assignments: %v", err)
	}
	if len(assigned.Assignments) != 4 {
		t.Fatalf("assignments = %+v", assigned.Assignments)
	}

	other, err := proxies.Create(t.Context(), &user.ID, "Other", nil)
	if err != nil {
		t.Fatalf("create second proxy: %v", err)
	}
	_, err = proxies.ReplaceAssignments(t.Context(), other.ID, []ProxyAssignment{
		{ScopeKind: ProxyScopeWorkspace, ScopeID: workspaceID},
	})
	var conflict *problem.Error
	if !errors.As(err, &conflict) || conflict.Code != "proxy_scope_taken" {
		t.Fatalf("conflicting assignment error = %v", err)
	}
	if _, err = proxies.ReplaceAssignments(t.Context(), other.ID, []ProxyAssignment{
		{ScopeKind: ProxyScopeWorkspace, ScopeID: uuid.NewString()},
	}); err == nil {
		t.Fatal("assignment to a missing workspace was accepted")
	}
	if _, err = proxies.ReplaceAssignments(t.Context(), other.ID, []ProxyAssignment{
		{ScopeKind: "environment", ScopeID: uuid.NewString()},
	}); err == nil {
		t.Fatal("assignment with an unknown scope kind was accepted")
	}

	excluded, err := proxies.ReplaceExclusions(t.Context(), proxy.ID, []string{user.ID}, []string{role.ID})
	if err != nil {
		t.Fatalf("replace exclusions: %v", err)
	}
	if len(excluded.ExcludedUserIDs) != 1 || excluded.ExcludedUserIDs[0] != user.ID ||
		len(excluded.ExcludedRoleIDs) != 1 || excluded.ExcludedRoleIDs[0] != role.ID {
		t.Fatalf("exclusions = %+v", excluded)
	}
	if _, err = proxies.ReplaceExclusions(t.Context(), proxy.ID, []string{uuid.NewString()}, nil); err == nil {
		t.Fatal("exclusion of a missing user was accepted")
	}

	if _, err = proxies.Delete(t.Context(), proxy.ID); err != nil {
		t.Fatalf("delete proxy: %v", err)
	}
	var assignmentCount, exclusionCount int64
	if err := db.Model(&ProxyAssignmentRecord{}).Where("proxy_id = ?", proxy.ID).Count(&assignmentCount).Error; err != nil {
		t.Fatalf("count assignments: %v", err)
	}
	if err := db.Model(&ProxyExclusionRecord{}).Where("proxy_id = ?", proxy.ID).Count(&exclusionCount).Error; err != nil {
		t.Fatalf("count exclusions: %v", err)
	}
	if assignmentCount != 0 || exclusionCount != 0 {
		t.Fatalf("dangling proxy rows: assignments=%d exclusions=%d", assignmentCount, exclusionCount)
	}
	if _, err = proxies.Get(t.Context(), proxy.ID); err == nil {
		t.Fatal("deleted proxy is still readable")
	}
}

func TestEffectiveOverridesLayeringAndExclusionFallThrough(t *testing.T) {
	db, proxies, _ := newProxyTestRepositories(t)
	workspaceID, collectionID, requestID := createProxyScopeFixtures(t, db)

	assign := func(name string, rules []HostnameOverride, scope ProxyAssignment) Proxy {
		t.Helper()
		proxy, err := proxies.Create(t.Context(), nil, name, rules)
		if err != nil {
			t.Fatalf("create %s: %v", name, err)
		}
		if _, err := proxies.ReplaceAssignments(t.Context(), proxy.ID, []ProxyAssignment{scope}); err != nil {
			t.Fatalf("assign %s: %v", name, err)
		}
		return proxy
	}

	assign("Server", []HostnameOverride{
		{Hostname: "a.demo", Target: "1.1.1.1"},
		{Hostname: "b.demo", Target: "2.2.2.2"},
	}, ProxyAssignment{ScopeKind: ProxyScopeServer})
	workspaceProxy := assign("Workspace", []HostnameOverride{
		{Hostname: "a.demo", Target: "3.3.3.3"},
	}, ProxyAssignment{ScopeKind: ProxyScopeWorkspace, ScopeID: workspaceID})
	assign("Collection", []HostnameOverride{
		{Hostname: "c.demo", Target: "4.4.4.4"},
	}, ProxyAssignment{ScopeKind: ProxyScopeCollection, ScopeID: collectionID})
	requestProxy := assign("Request", []HostnameOverride{
		{Hostname: "a.demo", Target: "5.5.5.5"},
	}, ProxyAssignment{ScopeKind: ProxyScopeRequest, ScopeID: requestID})

	chain := []ProxyScopeRef{
		{Kind: ProxyScopeRequest, ID: requestID},
		{Kind: ProxyScopeCollection, ID: collectionID},
		{Kind: ProxyScopeWorkspace, ID: workspaceID},
		{Kind: ProxyScopeServer, ID: ""},
	}
	userID := uuid.NewString()
	roleID := uuid.NewString()

	overrides, err := proxies.EffectiveOverrides(t.Context(), chain, userID, []string{roleID})
	if err != nil {
		t.Fatalf("resolve overrides: %v", err)
	}
	expect := func(overrides map[string]hostnameOverrideTarget, hostname, host string) {
		t.Helper()
		target, ok := overrides[hostname]
		if !ok || target.Host != host {
			t.Fatalf("override for %s = %+v (found %v), want %s", hostname, target, ok, host)
		}
	}
	// The most specific scope wins per hostname; unclaimed hostnames layer in
	// from less specific scopes.
	expect(overrides, "a.demo", "5.5.5.5")
	expect(overrides, "b.demo", "2.2.2.2")
	expect(overrides, "c.demo", "4.4.4.4")

	// A user excluded from the request proxy falls through to the workspace
	// proxy for the contested hostname.
	now := time.Now().UTC()
	excludedUser := identity.User{
		ID: userID, Email: "excluded", DisplayName: "Excluded", PasswordHash: "unused",
		Active: true, CreatedAt: now, UpdatedAt: now,
	}
	excludedRole := identity.Role{
		ID: roleID, Name: "Excluded role", NormalizedName: "excluded role", CreatedAt: now, UpdatedAt: now,
	}
	if err := db.Create(&excludedUser).Error; err != nil {
		t.Fatalf("create excluded user: %v", err)
	}
	if err := db.Create(&excludedRole).Error; err != nil {
		t.Fatalf("create excluded role: %v", err)
	}
	if _, err := proxies.ReplaceExclusions(t.Context(), requestProxy.ID, []string{userID}, nil); err != nil {
		t.Fatalf("exclude user from request proxy: %v", err)
	}
	overrides, err = proxies.EffectiveOverrides(t.Context(), chain, userID, []string{roleID})
	if err != nil {
		t.Fatalf("resolve overrides after user exclusion: %v", err)
	}
	expect(overrides, "a.demo", "3.3.3.3")
	expect(overrides, "c.demo", "4.4.4.4")

	// A role exclusion on the workspace proxy drops the next layer too, so the
	// hostname falls all the way through to the server-wide proxy.
	if _, err := proxies.ReplaceExclusions(t.Context(), workspaceProxy.ID, nil, []string{roleID}); err != nil {
		t.Fatalf("exclude role from workspace proxy: %v", err)
	}
	overrides, err = proxies.EffectiveOverrides(t.Context(), chain, userID, []string{roleID})
	if err != nil {
		t.Fatalf("resolve overrides after role exclusion: %v", err)
	}
	expect(overrides, "a.demo", "1.1.1.1")

	// A different user is unaffected by the exclusions.
	overrides, err = proxies.EffectiveOverrides(t.Context(), chain, uuid.NewString(), nil)
	if err != nil {
		t.Fatalf("resolve overrides for another user: %v", err)
	}
	expect(overrides, "a.demo", "5.5.5.5")
}

func TestAdoptLegacyOverridesCreatesServerDefaultProxyOnce(t *testing.T) {
	db, proxies, settings := newProxyTestRepositories(t)
	if err := db.Create(&SettingsRecord{ID: SettingsRecordID, Mode: ModeServer}).Error; err != nil {
		t.Fatalf("seed settings: %v", err)
	}
	if err := db.Create(&HostnameOverrideRecord{Hostname: "legacy.internal", Target: "http://127.0.0.1"}).Error; err != nil {
		t.Fatalf("seed legacy override: %v", err)
	}

	if err := proxies.AdoptLegacyOverrides(t.Context(), settings); err != nil {
		t.Fatalf("adopt legacy overrides: %v", err)
	}
	adopted, err := proxies.List(t.Context())
	if err != nil {
		t.Fatalf("list proxies: %v", err)
	}
	if len(adopted) != 1 || adopted[0].Name != "Server default" ||
		len(adopted[0].Rules) != 1 ||
		adopted[0].Rules[0] != (HostnameOverride{Hostname: "legacy.internal", Target: "http://127.0.0.1"}) {
		t.Fatalf("adopted proxies = %+v", adopted)
	}
	if len(adopted[0].Assignments) != 1 || adopted[0].Assignments[0].ScopeKind != ProxyScopeServer {
		t.Fatalf("adopted assignments = %+v", adopted[0].Assignments)
	}
	var legacyCount int64
	if err := db.Model(&HostnameOverrideRecord{}).Count(&legacyCount).Error; err != nil {
		t.Fatalf("count legacy overrides: %v", err)
	}
	if legacyCount != 0 {
		t.Fatalf("legacy override rows remain = %d", legacyCount)
	}

	// Adoption is idempotent: cleared legacy stores leave nothing to adopt.
	if err := proxies.AdoptLegacyOverrides(t.Context(), settings); err != nil {
		t.Fatalf("re-run adoption: %v", err)
	}
	remaining, err := proxies.List(t.Context())
	if err != nil {
		t.Fatalf("list proxies after re-run: %v", err)
	}
	if len(remaining) != 1 {
		t.Fatalf("proxies after re-run = %d, want 1", len(remaining))
	}
}

func TestAdoptLegacyOverridesReadsEncryptedBlob(t *testing.T) {
	db, proxies, settings := newProxyTestRepositories(t)
	if err := db.Create(&SettingsRecord{ID: SettingsRecordID, Mode: ModeServer}).Error; err != nil {
		t.Fatalf("seed settings: %v", err)
	}
	plaintext, err := json.Marshal([]HostnameOverride{{Hostname: "blob.internal", Target: "10.1.2.3"}})
	if err != nil {
		t.Fatalf("encode legacy blob: %v", err)
	}
	ciphertext, err := proxies.dataCipher.Encrypt(
		t.Context(), db, security.DeploymentDataScope(), "request_hostname_overrides", SettingsRecordID, plaintext,
	)
	if err != nil {
		t.Fatalf("encrypt legacy blob: %v", err)
	}
	if err := db.Model(&SettingsRecord{}).Where("id = ?", SettingsRecordID).
		Update("overrides_ciphertext", ciphertext).Error; err != nil {
		t.Fatalf("store legacy blob: %v", err)
	}

	if err := proxies.AdoptLegacyOverrides(t.Context(), settings); err != nil {
		t.Fatalf("adopt legacy overrides: %v", err)
	}
	adopted, err := proxies.List(t.Context())
	if err != nil {
		t.Fatalf("list proxies: %v", err)
	}
	if len(adopted) != 1 ||
		len(adopted[0].Rules) != 1 ||
		adopted[0].Rules[0] != (HostnameOverride{Hostname: "blob.internal", Target: "10.1.2.3"}) {
		t.Fatalf("adopted proxies = %+v", adopted)
	}
	var record SettingsRecord
	if err := db.First(&record, "id = ?", SettingsRecordID).Error; err != nil {
		t.Fatalf("load settings record: %v", err)
	}
	if len(record.OverridesCiphertext) != 0 {
		t.Fatal("legacy overrides ciphertext was not cleared")
	}
}
