package server_test

import (
	"bytes"
	"compress/gzip"
	"encoding/json"
	"io"
	"net"
	"net/http"
	"strings"
	"testing"
	"time"

	"resolved-server/internal/executionlimits"
	"resolved-server/internal/identity"
	"resolved-server/internal/users"
	"resolved-server/internal/workspaces"

	"github.com/gofiber/fiber/v3"
	gorillaWebsocket "github.com/gorilla/websocket"
)

func TestExecutionEnvelopeStreamsBeyondOldTransportCeiling(t *testing.T) {
	app, userService, _, cleanup := newTestServer(t)
	defer cleanup()
	owner, err := userService.BootstrapOwner(t.Context(), users.CreateInput{
		Email: "large-envelope-owner", DisplayName: "Owner", Password: ownerPassword,
	})
	if err != nil {
		t.Fatal(err)
	}
	token := login(t, app, owner.Email, ownerPassword).Token
	workspace := request[workspaces.WorkspaceView](t, app, http.MethodPost, "/api/v1/workspaces", token,
		map[string]any{"name": "Streamed execution"}, fiber.StatusCreated).Data
	endpoint := "/api/v1/request-execution/limits?workspace_id=" + workspace.ID
	request[any](t, app, http.MethodPut, endpoint, token,
		map[string]any{"overrides": executionlimits.Limits{
			"http.envelope_bytes": {Unlimited: true},
		}}, fiber.StatusOK)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	serverErr := make(chan error, 1)
	go func() {
		serverErr <- app.Listener(listener, fiber.ListenConfig{DisableStartupMessage: true})
	}()
	defer func() {
		if err := app.ShutdownWithTimeout(2 * time.Second); err != nil {
			t.Error(err)
		}
		if err := <-serverErr; err != nil {
			t.Error(err)
		}
	}()
	// The deliberately missing method prevents any target execution. Reaching its
	// validation error proves the full >96 MiB JSON envelope was streamed and
	// decoded, instead of being rejected by Fiber's previous global ceiling.
	body := io.MultiReader(
		strings.NewReader(`{"url":"https://example.invalid","padding":"`),
		io.LimitReader(repeatedJSONByte{}, 97*1024*1024),
		strings.NewReader(`"}`),
	)
	req, err := http.NewRequest(http.MethodPost,
		"http://"+listener.Addr().String()+"/api/v1/workspaces/"+workspace.ID+"/execute", body)
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+token)
	response, err := (&http.Client{Timeout: 30 * time.Second}).Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	var payload map[string]any
	if err := json.NewDecoder(response.Body).Decode(&payload); err != nil {
		t.Fatal(err)
	}
	if response.StatusCode != fiber.StatusUnprocessableEntity {
		t.Fatalf("status=%d payload=%v", response.StatusCode, payload)
	}
}

type repeatedJSONByte struct{}

func (repeatedJSONByte) Read(buffer []byte) (int, error) {
	for i := range buffer {
		buffer[i] = 'x'
	}
	return len(buffer), nil
}

func TestCompressedExecutionEnvelopeUsesDecodedLimit(t *testing.T) {
	app, userService, _, cleanup := newTestServer(t)
	defer cleanup()
	owner, err := userService.BootstrapOwner(t.Context(), users.CreateInput{
		Email: "compressed-owner", DisplayName: "Owner", Password: ownerPassword,
	})
	if err != nil {
		t.Fatal(err)
	}
	token := login(t, app, owner.Email, ownerPassword).Token
	workspace := request[workspaces.WorkspaceView](t, app, http.MethodPost, "/api/v1/workspaces", token,
		map[string]any{"name": "Compressed execution"}, fiber.StatusCreated).Data
	var compressed bytes.Buffer
	writer := gzip.NewWriter(&compressed)
	_, _ = writer.Write([]byte(`{"url":"https://example.invalid","padding":"` + strings.Repeat("x", 2048) + `"}`))
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		bound  executionlimits.Bound
		status int
	}{
		{executionlimits.Bound{Value: 1024}, fiber.StatusRequestEntityTooLarge},
		{executionlimits.Bound{Unlimited: true}, fiber.StatusUnprocessableEntity},
	} {
		request[any](t, app, http.MethodPut,
			"/api/v1/request-execution/limits?workspace_id="+workspace.ID, token,
			map[string]any{"overrides": executionlimits.Limits{"http.envelope_bytes": tc.bound}}, fiber.StatusOK)
		req, err := http.NewRequest(http.MethodPost, "/api/v1/workspaces/"+workspace.ID+"/execute", bytes.NewReader(compressed.Bytes()))
		if err != nil {
			t.Fatal(err)
		}
		req.Header.Set("Authorization", "Bearer "+token)
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("Content-Encoding", "gzip")
		response, err := app.Test(req)
		if err != nil {
			t.Fatal(err)
		}
		response.Body.Close()
		if response.StatusCode != tc.status {
			t.Fatalf("bound=%+v status=%d want=%d", tc.bound, response.StatusCode, tc.status)
		}
	}
}

func TestWebSocketAdmissionLivesUntilUpgradedConnectionCloses(t *testing.T) {
	app, userService, _, cleanup := newTestServer(t)
	defer cleanup()
	owner, err := userService.BootstrapOwner(t.Context(), users.CreateInput{
		Email: "ws-limits-owner", DisplayName: "Owner", Password: ownerPassword,
	})
	if err != nil {
		t.Fatal(err)
	}
	token := login(t, app, owner.Email, ownerPassword).Token
	workspace := request[workspaces.WorkspaceView](t, app, http.MethodPost, "/api/v1/workspaces", token,
		map[string]any{"name": "Concurrent execution"}, fiber.StatusCreated).Data
	request[any](t, app, http.MethodPut, "/api/v1/request-execution/limits?workspace_id="+workspace.ID, token,
		map[string]any{"overrides": executionlimits.Limits{"websocket.concurrent_sessions": {Value: 1}}}, fiber.StatusOK)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	serverErr := make(chan error, 1)
	go func() {
		serverErr <- app.Listener(listener, fiber.ListenConfig{DisableStartupMessage: true})
	}()
	defer func() {
		if err := app.ShutdownWithTimeout(2 * time.Second); err != nil {
			t.Error(err)
		}
		if err := <-serverErr; err != nil {
			t.Error(err)
		}
	}()
	endpoint := "ws://" + listener.Addr().String() + "/api/v1/workspaces/" + workspace.ID + "/execute"
	headers := http.Header{"Authorization": []string{"Bearer " + token}}
	dialer := gorillaWebsocket.Dialer{HandshakeTimeout: time.Second}
	first, _, err := dialer.Dial(endpoint, headers)
	if err != nil {
		t.Fatal(err)
	}
	defer first.Close()
	// No opening descriptor yet: even an upgraded, still-initializing session
	// owns its admission. Fiber's adaptor has already returned at this point.
	second, response, err := dialer.Dial(endpoint, headers)
	if second != nil {
		second.Close()
	}
	if response != nil {
		response.Body.Close()
	}
	if err == nil || response == nil || response.StatusCode != fiber.StatusTooManyRequests {
		t.Fatalf("second session bypassed admission: response=%v err=%v", response, err)
	}
	first.Close()
	deadline := time.Now().Add(2 * time.Second)
	for {
		next, response, err := dialer.Dial(endpoint, headers)
		if response != nil {
			response.Body.Close()
		}
		if err == nil {
			next.Close()
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("closed session did not release admission: %v", err)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func TestExecutionLimitManagementAPI(t *testing.T) {
	app, userService, db, cleanup := newTestServer(t)
	defer cleanup()
	owner, err := userService.BootstrapOwner(t.Context(), users.CreateInput{Email: "limits-owner", DisplayName: "Owner", Password: ownerPassword})
	if err != nil {
		t.Fatal(err)
	}
	token := login(t, app, owner.Email, ownerPassword).Token
	const endpoint = "/api/v1/request-execution/limits"
	request[any](t, app, http.MethodGet, endpoint, "", nil, fiber.StatusUnauthorized)
	initial := request[executionlimits.Snapshot](t, app, http.MethodGet, endpoint, token, nil, fiber.StatusOK)
	if len(initial.Data.Definitions) != 25 || initial.Data.Effective["http.redirects"].Value != 10 {
		t.Fatalf("%+v", initial.Data)
	}
	request[executionlimits.Snapshot](t, app, http.MethodPut, endpoint, token, map[string]any{"overrides": executionlimits.Limits{"http.redirects": {Unlimited: true}}}, fiber.StatusOK)
	snapshot, err := executionlimits.NewProvider(db).Resolve(t.Context(), executionlimits.Scope{})
	if err != nil || !snapshot.Effective["http.redirects"].Unlimited {
		t.Fatalf("%+v %v", snapshot, err)
	}
	for _, body := range []any{
		map[string]any{},
		map[string]any{"overrides": nil},
		map[string]any{"overrides": map[string]any{}, "typo": true},
	} {
		request[any](t, app, http.MethodPut, endpoint, token, body, fiber.StatusBadRequest)
	}
	request[any](t, app, http.MethodPut, endpoint, token, map[string]any{"overrides": executionlimits.Limits{"http.redirects": {Value: -1}}}, fiber.StatusUnprocessableEntity)

	workspace := request[workspaces.WorkspaceView](t, app, http.MethodPost, "/api/v1/workspaces", token, map[string]any{"name": "Limits scope"}, fiber.StatusCreated).Data
	root := request[workspaces.CollectionView](t, app, http.MethodPost, "/api/v1/workspaces/"+workspace.ID+"/collections", token, map[string]any{"name": "Root"}, fiber.StatusCreated).Data
	child := request[workspaces.CollectionView](t, app, http.MethodPost, "/api/v1/workspaces/"+workspace.ID+"/collections", token, map[string]any{"name": "Child", "parent_collection_id": root.ID}, fiber.StatusCreated).Data
	scoped := endpoint + "?workspace_id=" + workspace.ID
	childURL := scoped + "&collection_id=" + child.ID
	request[any](t, app, http.MethodPut, scoped+"&collection_id="+root.ID, token, map[string]any{"overrides": executionlimits.Limits{"http.redirects": {Value: 3}}}, fiber.StatusOK)
	inherited := request[executionlimits.Snapshot](t, app, http.MethodGet, childURL, token, nil, fiber.StatusOK).Data
	if inherited.Sources["http.redirects"].CollectionID != root.ID || len(inherited.Overrides) != 0 {
		t.Fatalf("%+v", inherited)
	}
	role := request[identity.RoleView](t, app, http.MethodPost, "/api/v1/roles", token, map[string]any{
		"name": "Limit editor", "permission_keys": []string{identity.PermissionCollectionsRead, identity.PermissionCollectionsUpdate, identity.PermissionWorkspacesRead, identity.PermissionWorkspacesUpdate},
	}, fiber.StatusCreated).Data
	member := request[identity.UserView](t, app, http.MethodPost, "/api/v1/users", token, map[string]any{
		"email": "limits-member", "display_name": "Member", "password": collaboratorPassword, "role_ids": []string{role.ID},
	}, fiber.StatusCreated).Data
	if err := db.Create(&workspaces.CollectionUser{CollectionID: child.ID, UserID: member.ID}).Error; err != nil {
		t.Fatal(err)
	}
	memberToken := login(t, app, member.Email, collaboratorPassword).Token
	request[any](t, app, http.MethodGet, endpoint, memberToken, nil, fiber.StatusForbidden)
	request[any](t, app, http.MethodGet, scoped, memberToken, nil, fiber.StatusForbidden)
	request[any](t, app, http.MethodGet, scoped+"&collection_id="+root.ID, memberToken, nil, fiber.StatusForbidden)
	request[any](t, app, http.MethodGet, childURL, memberToken, nil, fiber.StatusOK)
	request[any](t, app, http.MethodPut, childURL, memberToken, map[string]any{"overrides": executionlimits.Limits{"http.redirects": {Value: 0}}}, fiber.StatusOK)
	request[any](t, app, http.MethodPut, scoped, memberToken, map[string]any{"overrides": executionlimits.Limits{}}, fiber.StatusForbidden)
	request[any](t, app, http.MethodPut, childURL, memberToken, map[string]any{"overrides": executionlimits.Limits{}}, fiber.StatusOK)
	reset := request[executionlimits.Snapshot](t, app, http.MethodGet, childURL, memberToken, nil, fiber.StatusOK).Data
	if reset.Effective["http.redirects"].Value != 3 {
		t.Fatalf("%+v", reset)
	}
}
