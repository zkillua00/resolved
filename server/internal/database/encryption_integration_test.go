package database_test

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"testing"
	"time"

	"resolved-server/internal/activitylog"
	"resolved-server/internal/identity"
	"resolved-server/internal/requestproxy"
	"resolved-server/internal/security"
	"resolved-server/internal/sharedhistory"
	"resolved-server/internal/workspaces"

	"github.com/google/uuid"
	"gorm.io/driver/sqlite"
	"gorm.io/gorm"
)

func TestSensitiveRepositoriesPersistCiphertextOnly(t *testing.T) {
	db, dataCipher := encryptedRepositoryTestDatabase(t)
	now := time.Now().UTC()
	identityRepository := identity.NewRepository(db, dataCipher)
	user, err := identityRepository.CreateUser(t.Context(), identity.User{
		ID: uuid.NewString(), Email: "actor@example.test", DisplayName: "Audit Actor",
		PasswordHash: "unused", Active: true, CreatedAt: now, UpdatedAt: now,
	}, nil)
	if err != nil {
		t.Fatalf("create encrypted user: %v", err)
	}
	var rawUser identity.User
	if err := db.First(&rawUser, "id = ?", user.ID).Error; err != nil {
		t.Fatalf("load raw user: %v", err)
	}
	assertCiphertextOnly(
		t, rawUser.EncryptedProfile, rawUser.Email, rawUser.DisplayName,
		"actor@example.test", "Audit Actor",
	)
	if rawUser.EmailLookup == nil || *rawUser.EmailLookup == "" {
		t.Fatal("encrypted user does not have a blind login lookup")
	}
	role, err := identityRepository.CreateRole(t.Context(), identity.Role{
		ID: uuid.NewString(), Name: "Secret operators", NormalizedName: "secret operators",
		Description: "Access to private infrastructure", CreatedAt: now, UpdatedAt: now,
	}, nil)
	if err != nil {
		t.Fatalf("create encrypted role: %v", err)
	}
	if role.Name != "Secret operators" || role.Description != "Access to private infrastructure" {
		t.Fatalf("role round trip = %+v", role)
	}
	var rawRole identity.Role
	if err := db.First(&rawRole, "id = ?", role.ID).Error; err != nil {
		t.Fatalf("load raw role: %v", err)
	}
	assertCiphertextOnly(
		t, rawRole.EncryptedProfile, rawRole.Name, rawRole.NormalizedName, rawRole.Description,
		"Secret operators", "secret operators", "Access to private infrastructure",
	)
	foundUser, err := identityRepository.FindUserByEmail(t.Context(), "ACTOR@example.test")
	if err != nil || foundUser.ID != user.ID || foundUser.DisplayName != user.DisplayName {
		t.Fatalf("find encrypted user = %+v, err = %v", foundUser, err)
	}
	workspaceRepository := workspaces.NewRepository(db, dataCipher)
	environmentRepository := workspaces.NewEnvironmentRepository(workspaceRepository)
	workspace, err := workspaceRepository.CreateWorkspace(t.Context(), workspaces.Workspace{
		ID: uuid.NewString(), Name: "Workspace", CreatedAt: now, UpdatedAt: now,
	}, nil)
	if err != nil {
		t.Fatalf("create encrypted workspace: %v", err)
	}
	collection, err := workspaceRepository.CreateCollection(t.Context(), workspaces.Collection{
		ID: uuid.NewString(), WorkspaceID: workspace.ID, Name: "Collection", CreatedAt: now, UpdatedAt: now,
	})
	if err != nil {
		t.Fatalf("create encrypted collection: %v", err)
	}
	var rawWorkspace workspaces.Workspace
	if err := db.First(&rawWorkspace, "id = ?", workspace.ID).Error; err != nil {
		t.Fatalf("load raw workspace: %v", err)
	}
	assertCiphertextOnly(t, rawWorkspace.EncryptedName, rawWorkspace.Name, "Workspace")
	var rawCollection workspaces.Collection
	if err := db.First(&rawCollection, "id = ?", collection.ID).Error; err != nil {
		t.Fatalf("load raw collection: %v", err)
	}
	assertCiphertextOnly(t, rawCollection.EncryptedName, rawCollection.Name, "Collection")
	environment, err := environmentRepository.CreateEnvironment(t.Context(), workspaces.Environment{
		ID: uuid.NewString(), WorkspaceID: workspace.ID, Name: "Production secrets",
		CreatedAt: now, UpdatedAt: now,
	})
	if err != nil {
		t.Fatalf("create encrypted environment: %v", err)
	}
	variable, err := environmentRepository.CreateEnvironmentVariable(
		t.Context(), workspace.ID,
		workspaces.EnvironmentVariable{
			ID: uuid.NewString(), EnvironmentID: environment.ID, Key: "PRIVATE_API_TOKEN",
			Enabled: true, Secret: true, CreatedAt: now, UpdatedAt: now,
		},
		user.ID,
		[]byte("already-encrypted-user-value"),
	)
	if err != nil {
		t.Fatalf("create encrypted environment variable key: %v", err)
	}
	if variable.Key != "PRIVATE_API_TOKEN" {
		t.Fatalf("environment variable key round trip = %q", variable.Key)
	}
	var rawEnvironment workspaces.Environment
	if err := db.First(&rawEnvironment, "id = ?", environment.ID).Error; err != nil {
		t.Fatalf("load raw environment: %v", err)
	}
	assertCiphertextOnly(t, rawEnvironment.EncryptedName, rawEnvironment.Name, "Production secrets")
	var rawVariable workspaces.EnvironmentVariable
	if err := db.First(&rawVariable, "id = ?", variable.ID).Error; err != nil {
		t.Fatalf("load raw environment variable: %v", err)
	}
	assertCiphertextOnly(t, rawVariable.EncryptedKey, rawVariable.Key, "PRIVATE_API_TOKEN")
	if rawVariable.KeyLookup == nil || *rawVariable.KeyLookup == "" {
		t.Fatal("encrypted environment variable key does not have a blind uniqueness lookup")
	}

	saved, err := workspaceRepository.CreateSavedRequest(t.Context(), workspace.ID, workspaces.SavedRequest{
		ID: uuid.NewString(), CollectionID: collection.ID, Name: "Secret request",
		Definition: `{"request":{"url":"https://private.example.test","headers":[{"value":"token-value"}]}}`,
		CreatedAt:  now, UpdatedAt: now,
	})
	if err != nil {
		t.Fatalf("create encrypted saved request: %v", err)
	}
	if saved.Name != "Secret request" || !bytes.Contains([]byte(saved.Definition), []byte("token-value")) {
		t.Fatalf("saved request round trip = %+v", saved)
	}
	var rawRequest workspaces.SavedRequest
	if err := db.First(&rawRequest, "id = ?", saved.ID).Error; err != nil {
		t.Fatalf("load raw saved request: %v", err)
	}
	assertCiphertextOnly(t, rawRequest.EncryptedPayload, rawRequest.Name, rawRequest.Definition, "Secret request", "token-value")

	historyRepository := sharedhistory.NewRepository(db, dataCipher)
	historyClientEntryID := uuid.NewString()
	history, err := historyRepository.UpsertEntry(t.Context(), sharedhistory.Entry{
		ID: uuid.NewString(), WorkspaceID: workspace.ID, UserID: user.ID, ClientEntryID: historyClientEntryID,
		Method: "POST", URL: "https://private.example.test", RequestHeadersJSON: []byte(`[{"name":"authorization","value":"history-token"}]`),
		RequestBody: []byte(`{"secret":"history-body"}`), RequestBodyMode: "raw",
		RequestBodyFieldsJSON: []byte("[]"), ResponseHeadersJSON: []byte("[]"),
		CreatedAt: now, UpdatedAt: now,
	})
	if err != nil {
		t.Fatalf("create encrypted history: %v", err)
	}
	if history.ClientEntryID != historyClientEntryID || history.Method != "POST" ||
		!bytes.Contains(history.RequestBody, []byte("history-body")) {
		t.Fatalf("history round trip = %+v", history)
	}
	var rawHistory sharedhistory.Entry
	if err := db.First(&rawHistory, "id = ?", history.ID).Error; err != nil {
		t.Fatalf("load raw history: %v", err)
	}
	assertCiphertextOnly(
		t, rawHistory.EncryptedPayload, rawHistory.ClientEntryID, rawHistory.Method, rawHistory.URL,
		historyClientEntryID, "private.example.test", "history-token", "history-body",
	)
	if rawHistory.ClientEntryLookup == nil || *rawHistory.ClientEntryLookup == "" {
		t.Fatal("encrypted history client-entry ID does not have a blind uniqueness lookup")
	}

	diffs, _ := json.Marshal([]activitylog.DiffView{{Field: "definition", From: "before-secret", To: "after-secret"}})
	activityRepository := activitylog.NewRepository(db, dataCipher)
	activityID := uuid.NewString()
	if err := activityRepository.Create(t.Context(), activitylog.Entry{
		ID: activityID, Kind: activitylog.KindChange, Resource: "request", Action: "updated",
		ResourceID: saved.ID, WorkspaceID: workspace.ID, CollectionID: collection.ID,
		ActorUserID: user.ID, TargetName: "Secret request", DiffsJSON: diffs, CreatedAt: now,
	}); err != nil {
		t.Fatalf("create encrypted activity log: %v", err)
	}
	var rawActivity activitylog.Entry
	if err := db.First(&rawActivity, "id = ?", activityID).Error; err != nil {
		t.Fatalf("load raw activity: %v", err)
	}
	assertCiphertextOnly(
		t, rawActivity.EncryptedPayload, rawActivity.ActorEmail, rawActivity.ActorDisplayName,
		rawActivity.TargetName, string(rawActivity.DiffsJSON),
		"actor@example.test", "Audit Actor", "before-secret", "after-secret",
	)

	settingsRepository := requestproxy.NewSettingsRepository(db, dataCipher)
	if _, err := settingsRepository.Replace(t.Context(), requestproxy.Settings{
		Mode: requestproxy.ModeServer,
	}); err != nil {
		t.Fatalf("save encrypted request settings: %v", err)
	}
	if _, err := settingsRepository.AddAllowlistEntry(t.Context(), requestproxy.AllowlistEntry{
		Kind: "request", Value: "http://127.0.0.1/private?token=allowlist-secret",
	}); err != nil {
		t.Fatalf("save encrypted request allowlist: %v", err)
	}
	gotSettings, err := settingsRepository.Get(t.Context())
	if err != nil {
		t.Fatalf("load encrypted request settings: %v", err)
	}
	if len(gotSettings.AllowlistedRequests) != 1 {
		t.Fatalf("request allowlist round trip = %+v", gotSettings.AllowlistedRequests)
	}
	var rawSettings requestproxy.SettingsRecord
	if err := db.First(&rawSettings, "id = ?", requestproxy.SettingsRecordID).Error; err != nil {
		t.Fatalf("load raw request settings: %v", err)
	}
	assertCiphertextOnly(
		t, rawSettings.AllowlistCiphertext, "", "127.0.0.1", "allowlist-secret",
	)

	proxyRepository := requestproxy.NewProxyRepository(db, dataCipher)
	proxy, err := proxyRepository.Create(t.Context(), &user.ID, "Private proxy", []requestproxy.HostnameOverride{{
		Hostname: "private.example.test", Target: "https://10.20.30.40",
	}})
	if err != nil {
		t.Fatalf("create encrypted proxy: %v", err)
	}
	gotProxy, err := proxyRepository.Get(t.Context(), proxy.ID)
	if err != nil {
		t.Fatalf("load encrypted proxy: %v", err)
	}
	if gotProxy.Name != "Private proxy" || len(gotProxy.Rules) != 1 ||
		gotProxy.Rules[0] != (requestproxy.HostnameOverride{Hostname: "private.example.test", Target: "https://10.20.30.40"}) {
		t.Fatalf("proxy round trip = %+v", gotProxy)
	}
	var rawProxy requestproxy.ProxyRecord
	if err := db.First(&rawProxy, "id = ?", proxy.ID).Error; err != nil {
		t.Fatalf("load raw proxy: %v", err)
	}
	assertCiphertextOnly(
		t, rawProxy.PayloadCiphertext, "", "", "",
		"Private proxy", "private.example.test", "10.20.30.40",
	)
}

func TestLegacySensitiveRowsAreEncryptedAndCleared(t *testing.T) {
	db, dataCipher := encryptedRepositoryTestDatabase(t)
	now := time.Now().UTC()
	workspace := workspaces.Workspace{ID: uuid.NewString(), Name: "Workspace", CreatedAt: now, UpdatedAt: now}
	collection := workspaces.Collection{
		ID: uuid.NewString(), WorkspaceID: workspace.ID, Name: "Collection", CreatedAt: now, UpdatedAt: now,
	}
	if err := db.Create(&workspace).Error; err != nil {
		t.Fatalf("create workspace: %v", err)
	}
	if err := db.Create(&collection).Error; err != nil {
		t.Fatalf("create collection: %v", err)
	}
	legacy := workspaces.SavedRequest{
		ID: uuid.NewString(), CollectionID: collection.ID, Name: "Legacy secret",
		Definition: `{"token":"legacy-token"}`, CreatedAt: now, UpdatedAt: now,
	}
	if err := db.Create(&legacy).Error; err != nil {
		t.Fatalf("create legacy saved request: %v", err)
	}
	repository := workspaces.NewRepository(db, dataCipher)
	if err := repository.EncryptLegacySavedRequests(t.Context()); err != nil {
		t.Fatalf("encrypt legacy saved request: %v", err)
	}
	var raw workspaces.SavedRequest
	if err := db.First(&raw, "id = ?", legacy.ID).Error; err != nil {
		t.Fatalf("load migrated request: %v", err)
	}
	assertCiphertextOnly(t, raw.EncryptedPayload, raw.Name, raw.Definition, "Legacy secret", "legacy-token")
	got, err := repository.GetSavedRequest(t.Context(), workspace.ID, collection.ID, legacy.ID)
	if err != nil {
		t.Fatalf("read migrated request: %v", err)
	}
	if got.Name != legacy.Name || got.Definition != legacy.Definition {
		t.Fatalf("migrated request = %+v, want %+v", got, legacy)
	}
}

func encryptedRepositoryTestDatabase(t *testing.T) (*gorm.DB, *security.DataCipher) {
	t.Helper()
	db, err := gorm.Open(sqlite.Open("file:"+uuid.NewString()+"?mode=memory&cache=shared&_foreign_keys=on"), &gorm.Config{})
	if err != nil {
		t.Fatalf("open database: %v", err)
	}
	sqlDB, err := db.DB()
	if err != nil {
		t.Fatalf("access database: %v", err)
	}
	t.Cleanup(func() { _ = sqlDB.Close() })
	if err := db.AutoMigrate(
		&security.DataEncryptionKey{}, &identity.Permission{}, &identity.Role{}, &identity.User{},
		&workspaces.Workspace{}, &workspaces.Collection{},
		&workspaces.SavedRequest{}, &workspaces.WorkspaceUser{}, &workspaces.CollectionUser{},
		&workspaces.Environment{}, &workspaces.EnvironmentVariable{}, &workspaces.EnvironmentVariableValue{},
		&sharedhistory.Entry{}, &activitylog.Entry{},
		&requestproxy.SettingsRecord{}, &requestproxy.HostnameOverrideRecord{},
		&requestproxy.ProxyRecord{}, &requestproxy.ProxyAssignmentRecord{}, &requestproxy.ProxyExclusionRecord{},
	); err != nil {
		t.Fatalf("migrate database: %v", err)
	}
	provider, err := security.NewStaticKeyProvider(
		"test-root-v1",
		base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0x33}, security.DataKeyLength)),
	)
	if err != nil {
		t.Fatalf("create key provider: %v", err)
	}
	dataCipher, err := security.NewDataCipher(provider)
	if err != nil {
		t.Fatalf("create data cipher: %v", err)
	}
	t.Cleanup(dataCipher.Close)
	return db, dataCipher
}

func assertCiphertextOnly(t *testing.T, ciphertext []byte, storedAndSecrets ...string) {
	t.Helper()
	if len(ciphertext) == 0 {
		t.Fatal("ciphertext is empty")
	}
	half := len(storedAndSecrets) / 2
	for _, stored := range storedAndSecrets[:half] {
		if stored != "" && stored != "[]" {
			t.Fatalf("plaintext column was not cleared: %q", stored)
		}
	}
	for _, secret := range storedAndSecrets[half:] {
		if bytes.Contains(ciphertext, []byte(secret)) {
			t.Fatalf("ciphertext contains plaintext %q", secret)
		}
	}
}
