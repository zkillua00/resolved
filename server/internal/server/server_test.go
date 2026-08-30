package server_test

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"
	"time"

	"resolved-server/eventsystem"
	"resolved-server/internal/activitylog"
	"resolved-server/internal/auth"
	"resolved-server/internal/config"
	"resolved-server/internal/database"
	"resolved-server/internal/identity"
	"resolved-server/internal/realtime"
	"resolved-server/internal/requestproxy"
	"resolved-server/internal/resourceevents"
	"resolved-server/internal/roles"
	"resolved-server/internal/security"
	"resolved-server/internal/server"
	"resolved-server/internal/sharedhistory"
	"resolved-server/internal/users"
	"resolved-server/internal/workspaces"

	"github.com/gofiber/fiber/v3"
	"github.com/google/uuid"
	gorillaWebsocket "github.com/gorilla/websocket"
	"gorm.io/gorm"
)

const (
	ownerPassword        = "owner password for tests"
	collaboratorPassword = "collaborator password"
)

type apiResponse[Data any] struct {
	RequestID string `json:"request_id"`
	Success   bool   `json:"success"`
	Data      Data   `json:"data"`
	Error     struct {
		Code    string            `json:"code"`
		Message string            `json:"message"`
		Fields  map[string]string `json:"fields"`
	} `json:"error"`
}

func TestIdentityManagementAndDynamicPermissions(t *testing.T) {
	app, usersService, _, closeDatabase := newTestServer(t)
	defer closeDatabase()

	owner, err := usersService.BootstrapOwner(t.Context(), users.CreateInput{
		Email:       "owner",
		DisplayName: "Owner",
		Password:    ownerPassword,
	})
	if err != nil {
		t.Fatalf("bootstrap owner: %v", err)
	}
	if _, err := usersService.BootstrapOwner(t.Context(), users.CreateInput{
		Email:       "second-owner",
		DisplayName: "Second owner",
		Password:    ownerPassword,
	}); err == nil {
		t.Fatal("expected a second bootstrap attempt to be rejected")
	}
	unauthorized := request[any](t, app, http.MethodGet, "/api/v1/users", "", nil, fiber.StatusUnauthorized)
	if unauthorized.Error.Code != "unauthorized" || unauthorized.RequestID == "" {
		t.Fatalf("unexpected unauthorized response: %+v", unauthorized)
	}

	invalidLogin := request[auth.LoginResponse](t, app, http.MethodPost, "/api/v1/auth/login", "", map[string]any{
		"email":    owner.Email,
		"password": "wrong but sufficiently long",
	}, fiber.StatusUnauthorized)
	if invalidLogin.Error.Code != "invalid_credentials" {
		t.Fatalf("invalid login code = %q, want invalid_credentials", invalidLogin.Error.Code)
	}

	ownerLogin := login(t, app, owner.Email, ownerPassword)
	initialWorkspaces := request[[]workspaces.WorkspaceView](
		t, app, http.MethodGet, "/api/v1/workspaces", ownerLogin.Token, nil, fiber.StatusOK,
	).Data
	if len(initialWorkspaces) != 1 || initialWorkspaces[0].Name != workspaces.DefaultWorkspaceName {
		t.Fatalf("initial workspaces = %+v, want one %q workspace", initialWorkspaces, workspaces.DefaultWorkspaceName)
	}
	if len(initialWorkspaces[0].UserIDs) != 1 || initialWorkspaces[0].UserIDs[0] != owner.ID {
		t.Fatalf("initial workspace users = %v, want [%s]", initialWorkspaces[0].UserIDs, owner.ID)
	}
	assertCreator(t, initialWorkspaces[0].CreatedBy, owner.ID, owner.Email)
	invalidUser := request[identity.UserView](t, app, http.MethodPost, "/api/v1/users", ownerLogin.Token, map[string]any{
		"email":        "not-an-email",
		"display_name": "Invalid",
		"password":     "short",
		"role_ids":     []string{},
	}, fiber.StatusUnprocessableEntity)
	if invalidUser.Error.Code != "validation_failed" || invalidUser.Error.Fields["email"] != "" || invalidUser.Error.Fields["password"] == "" {
		t.Fatalf("unexpected validation response: %+v", invalidUser.Error)
	}

	roleResponse := request[identity.RoleView](t, app, http.MethodPost, "/api/v1/roles", ownerLogin.Token, map[string]any{
		"name":            "Collaborator",
		"description":     "Can initially view users",
		"permission_keys": []string{identity.PermissionUsersRead},
	}, fiber.StatusCreated)
	if roleResponse.Data.ID == "" {
		t.Fatal("created role did not include an ID")
	}
	assertCreator(t, roleResponse.Data.CreatedBy, owner.ID, owner.Email)

	userResponse := request[identity.UserView](t, app, http.MethodPost, "/api/v1/users", ownerLogin.Token, map[string]any{
		"email":        "collaborator",
		"display_name": "Collaborator",
		"password":     collaboratorPassword,
		"role_ids":     []string{roleResponse.Data.ID},
	}, fiber.StatusCreated)
	assertCreator(t, userResponse.Data.CreatedBy, owner.ID, owner.Email)
	collaboratorLogin := login(t, app, userResponse.Data.Email, collaboratorPassword)

	request[[]identity.UserView](t, app, http.MethodGet, "/api/v1/users", collaboratorLogin.Token, nil, fiber.StatusOK)
	forbidden := request[identity.UserView](t, app, http.MethodPost, "/api/v1/users", collaboratorLogin.Token, map[string]any{
		"email":        "before-permission",
		"display_name": "Before permission",
		"password":     collaboratorPassword,
		"role_ids":     []string{},
	}, fiber.StatusForbidden)
	if forbidden.Error.Code != "forbidden" {
		t.Fatalf("forbidden code = %q, want forbidden", forbidden.Error.Code)
	}

	request[identity.RoleView](t, app, http.MethodPut, "/api/v1/roles/"+roleResponse.Data.ID+"/permissions", ownerLogin.Token, map[string]any{
		"permission_keys": []string{identity.PermissionUsersRead, identity.PermissionUsersCreate},
	}, fiber.StatusOK)

	createdAfterPermission := request[identity.UserView](t, app, http.MethodPost, "/api/v1/users", collaboratorLogin.Token, map[string]any{
		"email":        "after-permission",
		"display_name": "After permission",
		"password":     collaboratorPassword,
		"role_ids":     []string{},
	}, fiber.StatusCreated)
	if createdAfterPermission.Data.Email != "after-permission" {
		t.Fatalf("created email = %q", createdAfterPermission.Data.Email)
	}
	assertCreator(t, createdAfterPermission.Data.CreatedBy, userResponse.Data.ID, userResponse.Data.Email)

	immutableOwner := request[identity.RoleView](t, app, http.MethodPut, "/api/v1/roles/"+identity.OwnerRoleID+"/permissions", ownerLogin.Token, map[string]any{
		"permission_keys": []string{},
	}, fiber.StatusForbidden)
	if immutableOwner.Error.Code != "system_role_immutable" {
		t.Fatalf("owner update code = %q, want system_role_immutable", immutableOwner.Error.Code)
	}

	lastOwner := request[identity.UserView](t, app, http.MethodPut, "/api/v1/users/"+owner.ID+"/roles", ownerLogin.Token, map[string]any{
		"role_ids": []string{},
	}, fiber.StatusConflict)
	if lastOwner.Error.Code != "last_owner" {
		t.Fatalf("last owner code = %q, want last_owner", lastOwner.Error.Code)
	}

	request[struct{}](t, app, http.MethodPost, "/api/v1/auth/logout", collaboratorLogin.Token, nil, fiber.StatusOK)
	request[any](t, app, http.MethodGet, "/api/v1/users", collaboratorLogin.Token, nil, fiber.StatusUnauthorized)
}

func TestRequestProxyExecutesFromServerWithDynamicPermissionAndWorkspaceScope(t *testing.T) {
	app, usersService, _, closeDatabase := newTestServer(t)
	defer closeDatabase()

	owner, err := usersService.BootstrapOwner(t.Context(), users.CreateInput{
		Email:       "owner",
		DisplayName: "Owner",
		Password:    ownerPassword,
	})
	if err != nil {
		t.Fatalf("bootstrap owner: %v", err)
	}
	ownerLogin := login(t, app, owner.Email, ownerPassword)
	workspace := request[[]workspaces.WorkspaceView](
		t, app, http.MethodGet, "/api/v1/workspaces", ownerLogin.Token, nil, fiber.StatusOK,
	).Data[0]

	role := request[identity.RoleView](t, app, http.MethodPost, "/api/v1/roles", ownerLogin.Token, map[string]any{
		"name":            "Request runner",
		"description":     "Can be granted proxied request execution",
		"permission_keys": []string{},
	}, fiber.StatusCreated).Data
	user := request[identity.UserView](t, app, http.MethodPost, "/api/v1/users", ownerLogin.Token, map[string]any{
		"email":        "runner",
		"display_name": "Runner",
		"password":     collaboratorPassword,
		"role_ids":     []string{role.ID},
	}, fiber.StatusCreated).Data
	request[workspaces.WorkspaceView](
		t,
		app,
		http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/users",
		ownerLogin.Token,
		map[string]any{"user_ids": []string{owner.ID, user.ID}},
		fiber.StatusOK,
	)
	runnerLogin := login(t, app, user.Email, collaboratorPassword)
	defaultPolicy := request[requestproxy.Policy](
		t, app, http.MethodGet, "/api/v1/request-execution", runnerLogin.Token, nil, fiber.StatusOK,
	).Data
	if defaultPolicy.Mode != requestproxy.ModeLocal {
		t.Fatalf("default request execution mode = %q, want local", defaultPolicy.Mode)
	}
	disabled := request[requestproxy.ExecuteResult](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+workspace.ID+"/execute",
		ownerLogin.Token,
		map[string]any{
			"method": "GET",
			"url":    "http://127.0.0.1:1/must-not-connect",
			"body":   map[string]any{"mode": "none"},
		},
		fiber.StatusConflict,
	)
	if disabled.Error.Code != "server_execution_disabled" {
		t.Fatalf("disabled execution error = %q", disabled.Error.Code)
	}
	request[requestproxy.Settings](
		t,
		app,
		http.MethodGet,
		"/api/v1/request-execution/settings",
		runnerLogin.Token,
		nil,
		fiber.StatusForbidden,
	)

	targetAuthorization := make(chan string, 1)
	targetHost := make(chan string, 1)
	target := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, incoming *http.Request) {
		body, readErr := io.ReadAll(incoming.Body)
		if readErr != nil {
			t.Errorf("read proxied request: %v", readErr)
		}
		if incoming.Method != http.MethodPatch {
			t.Errorf("proxied method = %s, want PATCH", incoming.Method)
		}
		if string(body) != `{"proxied":true}` {
			t.Errorf("proxied body = %q", body)
		}
		if incoming.Header.Get("X-Resolved-Test") != "yes" {
			t.Errorf("proxied header = %q", incoming.Header.Get("X-Resolved-Test"))
		}
		if incoming.Header.Get("Content-Type") != "application/custom+json" {
			t.Errorf("proxied content type = %q", incoming.Header.Get("Content-Type"))
		}
		targetAuthorization <- incoming.Header.Get("Authorization")
		targetHost <- incoming.Host
		writer.Header().Add("X-Target", "one")
		writer.Header().Add("X-Target", "two")
		writer.Header().Set("Content-Type", "application/octet-stream")
		writer.WriteHeader(http.StatusCreated)
		_, _ = writer.Write([]byte{0, 1, 2, 255})
	}))
	defer target.Close()
	targetPort := target.Listener.Addr().(*net.TCPAddr).Port
	targetURL := fmt.Sprintf("service.internal:%d/resource", targetPort)
	expectedTargetURL := "http://" + targetURL
	settings := request[requestproxy.Settings](
		t,
		app,
		http.MethodPut,
		"/api/v1/request-execution/settings",
		ownerLogin.Token,
		map[string]any{
			"mode": requestproxy.ModeServer,
			"hostname_overrides": []map[string]string{
				{"hostname": "SERVICE.INTERNAL.", "target": "http://127.0.0.1"},
			},
		},
		fiber.StatusOK,
	).Data
	if settings.Mode != requestproxy.ModeServer || len(settings.HostnameOverrides) != 1 || settings.HostnameOverrides[0] != (requestproxy.HostnameOverride{Hostname: "service.internal", Target: "http://127.0.0.1"}) {
		t.Fatalf("normalized request execution settings = %+v", settings)
	}
	serverPolicy := request[requestproxy.Policy](
		t, app, http.MethodGet, "/api/v1/request-execution", runnerLogin.Token, nil, fiber.StatusOK,
	).Data
	if serverPolicy.Mode != requestproxy.ModeServer {
		t.Fatalf("updated request execution mode = %q, want server", serverPolicy.Mode)
	}

	proxyPayload := map[string]any{
		"method": "PATCH",
		"url":    targetURL,
		"headers": []map[string]string{
			{"name": "X-Resolved-Test", "value": "yes"},
			{"name": "Content-Type", "value": "application/custom+json"},
		},
		"body": map[string]any{
			"mode":             "raw",
			"raw_content_type": "application/json",
			"data_base64":      base64.StdEncoding.EncodeToString([]byte(`{"proxied":true}`)),
		},
	}
	forbidden := request[requestproxy.ExecuteResult](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+workspace.ID+"/execute",
		runnerLogin.Token,
		proxyPayload,
		fiber.StatusForbidden,
	)
	if forbidden.Error.Code != "forbidden" {
		t.Fatalf("proxy permission error = %q, want forbidden", forbidden.Error.Code)
	}

	request[identity.RoleView](t, app, http.MethodPut, "/api/v1/roles/"+role.ID+"/permissions", ownerLogin.Token, map[string]any{
		"permission_keys": []string{identity.PermissionRequestsExecute},
	}, fiber.StatusOK)
	result := request[requestproxy.ExecuteResult](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+workspace.ID+"/execute",
		runnerLogin.Token,
		proxyPayload,
		fiber.StatusOK,
	).Data
	if result.Status != http.StatusCreated || result.StatusText != "Created" {
		t.Fatalf("target status = %d %q", result.Status, result.StatusText)
	}
	if result.FinalURL != expectedTargetURL || result.HTTPVersion == "" || result.DurationMicros < 0 {
		t.Fatalf("unexpected proxy metadata: %+v", result)
	}
	decodedBody, err := base64.StdEncoding.DecodeString(result.BodyBase64)
	if err != nil {
		t.Fatalf("decode proxied response: %v", err)
	}
	if !bytes.Equal(decodedBody, []byte{0, 1, 2, 255}) {
		t.Fatalf("proxied response body = %v", decodedBody)
	}
	var targetHeaders []string
	for _, header := range result.Headers {
		if header.Name == "X-Target" {
			targetHeaders = append(targetHeaders, header.Value)
		}
	}
	if len(targetHeaders) != 2 {
		t.Fatalf("repeated target response headers = %v", targetHeaders)
	}
	if authorization := <-targetAuthorization; authorization != "" {
		t.Fatalf("server bearer token leaked to target as %q", authorization)
	}
	if host := <-targetHost; host != fmt.Sprintf("service.internal:%d", targetPort) {
		t.Fatalf("custom hostname changed target origin to %q", host)
	}

	otherWorkspace := request[workspaces.WorkspaceView](t, app, http.MethodPost, "/api/v1/workspaces", ownerLogin.Token, map[string]any{
		"name": "Private",
	}, fiber.StatusCreated).Data
	deniedByScope := request[requestproxy.ExecuteResult](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+otherWorkspace.ID+"/execute",
		runnerLogin.Token,
		proxyPayload,
		fiber.StatusForbidden,
	)
	if deniedByScope.Error.Code != "workspace_access_denied" {
		t.Fatalf("proxy scope error = %q, want workspace_access_denied", deniedByScope.Error.Code)
	}
}

func TestAuthenticatedWebsocketPublishesResourceChanges(t *testing.T) {
	app, usersService, _, closeDatabase := newTestServer(t)
	t.Cleanup(closeDatabase)

	owner, err := usersService.BootstrapOwner(t.Context(), users.CreateInput{
		Email:       "owner",
		DisplayName: "Owner",
		Password:    ownerPassword,
	})
	if err != nil {
		t.Fatalf("bootstrap owner: %v", err)
	}
	ownerLogin := login(t, app, owner.Email, ownerPassword)

	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	serverErr := make(chan error, 1)
	go func() {
		serverErr <- app.Listener(listener, fiber.ListenConfig{DisableStartupMessage: true})
	}()
	t.Cleanup(func() {
		if err := app.ShutdownWithTimeout(2 * time.Second); err != nil {
			t.Errorf("shutdown Fiber app: %v", err)
		}
		select {
		case err := <-serverErr:
			if err != nil {
				t.Errorf("serve Fiber app: %v", err)
			}
		case <-time.After(2 * time.Second):
			t.Error("Fiber app did not stop")
		}
	})

	websocketURL := fmt.Sprintf("ws://%s/api/v1/ws", listener.Addr().String())
	unauthorized, response, err := gorillaWebsocket.DefaultDialer.Dial(websocketURL, nil)
	if unauthorized != nil {
		_ = unauthorized.Close()
	}
	if response != nil {
		defer response.Body.Close()
	}
	if err == nil || response == nil || response.StatusCode != fiber.StatusUnauthorized {
		t.Fatalf("unauthorized websocket response = %+v, error = %v", response, err)
	}

	headers := http.Header{"Authorization": []string{"Bearer " + ownerLogin.Token}}
	client, response, err := gorillaWebsocket.DefaultDialer.Dial(websocketURL, headers)
	if err != nil {
		if response != nil {
			t.Fatalf("dial authenticated websocket: %v (status %s)", err, response.Status)
		}
		t.Fatalf("dial authenticated websocket: %v", err)
	}
	defer client.Close()

	created := request[workspaces.WorkspaceView](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces",
		ownerLogin.Token,
		map[string]any{"name": "Realtime workspace"},
		fiber.StatusCreated,
	).Data
	if err := client.SetReadDeadline(time.Now().Add(2 * time.Second)); err != nil {
		t.Fatalf("set websocket deadline: %v", err)
	}
	var published struct {
		Command string                `json:"command"`
		Data    resourceevents.Change `json:"data"`
	}
	if err := client.ReadJSON(&published); err != nil {
		t.Fatalf("read resource change: %v", err)
	}
	if published.Command != resourceevents.EventName {
		t.Fatalf("command = %q, want %q", published.Command, resourceevents.EventName)
	}
	if published.Data.Resource != resourceevents.ResourceWorkspace ||
		published.Data.Action != resourceevents.ActionCreated ||
		published.Data.ResourceID != created.ID {
		t.Fatalf("unexpected resource change: %+v", published.Data)
	}

	collection := request[workspaces.CollectionView](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+created.ID+"/collections",
		ownerLogin.Token,
		map[string]any{"name": "Realtime collection"},
		fiber.StatusCreated,
	).Data
	if err := client.SetReadDeadline(time.Now().Add(2 * time.Second)); err != nil {
		t.Fatalf("set collection create websocket deadline: %v", err)
	}
	if err := client.ReadJSON(&published); err != nil {
		t.Fatalf("read collection create change: %v", err)
	}
	if published.Data.Resource != resourceevents.ResourceCollection ||
		published.Data.Action != resourceevents.ActionCreated ||
		published.Data.ResourceID != collection.ID ||
		published.Data.WorkspaceID != created.ID {
		t.Fatalf("unexpected collection create change: %+v", published.Data)
	}

	request[struct{}](
		t,
		app,
		http.MethodDelete,
		"/api/v1/workspaces/"+created.ID+"/collections/"+collection.ID,
		ownerLogin.Token,
		nil,
		fiber.StatusOK,
	)
	if err := client.SetReadDeadline(time.Now().Add(2 * time.Second)); err != nil {
		t.Fatalf("set collection delete websocket deadline: %v", err)
	}
	if err := client.ReadJSON(&published); err != nil {
		t.Fatalf("read collection delete change: %v", err)
	}
	if published.Data.Resource != resourceevents.ResourceCollection ||
		published.Data.Action != resourceevents.ActionDeleted ||
		published.Data.ResourceID != collection.ID ||
		published.Data.WorkspaceID != created.ID {
		t.Fatalf("unexpected collection delete change: %+v", published.Data)
	}

	request[sharedhistory.EntryView](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+created.ID+"/history",
		ownerLogin.Token,
		map[string]any{
			"client_entry_id": "realtime-history-1",
			"created_at":      time.Now().UTC(),
			"request": map[string]any{
				"method": "GET", "url": "https://example.test",
				"body_mode": "none",
			},
		},
		fiber.StatusCreated,
	)
	if err := client.SetReadDeadline(time.Now().Add(2 * time.Second)); err != nil {
		t.Fatalf("set shared history websocket deadline: %v", err)
	}
	if err := client.ReadJSON(&published); err != nil {
		t.Fatalf("read shared history change: %v", err)
	}
	if published.Data.Resource != resourceevents.ResourceSharedHistory ||
		published.Data.Action != resourceevents.ActionUpdated ||
		published.Data.ResourceID != owner.ID ||
		published.Data.WorkspaceID != created.ID {
		t.Fatalf("unexpected shared history change: %+v", published.Data)
	}
}

func TestSharedHistoryProfilesAndAuthorization(t *testing.T) {
	app, usersService, _, closeDatabase := newTestServer(t)
	t.Cleanup(closeDatabase)

	owner, err := usersService.BootstrapOwner(t.Context(), users.CreateInput{
		Email:       "owner",
		DisplayName: "Owner",
		Password:    ownerPassword,
	})
	if err != nil {
		t.Fatalf("bootstrap owner: %v", err)
	}
	ownerLogin := login(t, app, owner.Email, ownerPassword)
	workspace := request[[]workspaces.WorkspaceView](
		t, app, http.MethodGet, "/api/v1/workspaces", ownerLogin.Token, nil, fiber.StatusOK,
	).Data[0]

	role := request[identity.RoleView](t, app, http.MethodPost, "/api/v1/roles", ownerLogin.Token, map[string]any{
		"name":            "History member",
		"description":     "Can use personal shared history",
		"permission_keys": []string{},
	}, fiber.StatusCreated).Data
	member := request[identity.UserView](t, app, http.MethodPost, "/api/v1/users", ownerLogin.Token, map[string]any{
		"email":        "history-member",
		"display_name": "History Member",
		"password":     collaboratorPassword,
		"role_ids":     []string{role.ID},
	}, fiber.StatusCreated).Data
	request[workspaces.WorkspaceView](
		t,
		app,
		http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/users",
		ownerLogin.Token,
		map[string]any{"user_ids": []string{owner.ID, member.ID}},
		fiber.StatusOK,
	)
	memberLogin := login(t, app, member.Email, collaboratorPassword)

	profiles := request[[]sharedhistory.ProfileView](
		t, app, http.MethodGet, "/api/v1/profiles", memberLogin.Token, nil, fiber.StatusOK,
	).Data
	if len(profiles) != 2 {
		t.Fatalf("profiles = %+v, want owner and member", profiles)
	}

	responseBody := []byte{0, 1, 2, 255}
	created := request[sharedhistory.EntryView](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+workspace.ID+"/history",
		memberLogin.Token,
		map[string]any{
			"client_entry_id": "desktop-entry-1",
			"created_at":      time.Now().UTC(),
			"request": map[string]any{
				"method":            "POST",
				"url":               "https://api.example.test/widgets",
				"headers":           []map[string]any{{"name": "Content-Type", "value": "application/json"}},
				"body":              `{"name":"shared"}`,
				"body_mode":         "multipart_form_data",
				"raw_body_language": "json",
				"body_fields": []map[string]any{
					{"enabled": true, "name": "note", "value": "visible", "kind": "text"},
					{"enabled": true, "name": "upload", "value": "/private/file.txt", "kind": "file"},
				},
				"body_truncated": false,
			},
			"response": map[string]any{
				"status":          201,
				"status_text":     "Created",
				"http_version":    "HTTP/2",
				"final_url":       "https://api.example.test/widgets/1",
				"headers":         []map[string]any{{"name": "Content-Type", "value": "application/octet-stream"}},
				"body_base64":     base64.StdEncoding.EncodeToString(responseBody),
				"body_truncated":  false,
				"content_type":    "application/octet-stream",
				"duration_micros": 1250,
			},
		},
		fiber.StatusCreated,
	).Data
	if created.Request.Body != "" || len(created.Request.BodyFields) != 2 || created.Request.BodyFields[0].Value != "visible" {
		t.Fatalf("created shared request = %+v", created.Request)
	}
	if created.Request.BodyFields[1].Value != "" {
		t.Fatalf("file path was persisted in shared history: %+v", created.Request.BodyFields[1])
	}
	if created.Response == nil || created.Response.BodyBase64 != base64.StdEncoding.EncodeToString(responseBody) {
		t.Fatalf("created shared response = %+v", created.Response)
	}
	invalid := request[sharedhistory.EntryView](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+workspace.ID+"/history",
		memberLogin.Token,
		map[string]any{
			"client_entry_id": "invalid-duration",
			"created_at":      time.Now().UTC(),
			"request": map[string]any{
				"method": "GET", "url": "https://api.example.test/widgets",
				"headers": []any{}, "body": "", "body_mode": "none",
				"raw_body_language": "text", "body_fields": []any{}, "body_truncated": false,
			},
			"response": map[string]any{
				"status": 200, "status_text": "OK", "http_version": "HTTP/2",
				"final_url": "https://api.example.test/widgets", "headers": []any{},
				"body_base64": "", "body_truncated": false, "duration_micros": -1,
			},
		},
		fiber.StatusUnprocessableEntity,
	)
	if invalid.Error.Fields["response.duration_micros"] == "" {
		t.Fatalf("shared history validation error lost its field reason: %+v", invalid.Error)
	}

	personal := request[[]sharedhistory.EntryView](
		t,
		app,
		http.MethodGet,
		"/api/v1/profiles/"+member.ID+"/history?workspace_id="+workspace.ID,
		memberLogin.Token,
		nil,
		fiber.StatusOK,
	).Data
	if len(personal) != 1 || personal[0].ID != created.ID {
		t.Fatalf("personal history = %+v, want created entry", personal)
	}

	denied := request[[]sharedhistory.EntryView](
		t,
		app,
		http.MethodGet,
		"/api/v1/profiles/"+owner.ID+"/history?workspace_id="+workspace.ID,
		memberLogin.Token,
		nil,
		fiber.StatusForbidden,
	)
	if denied.Error.Code != "history_access_denied" {
		t.Fatalf("other-user history error = %+v, want history_access_denied", denied.Error)
	}

	request[identity.RoleView](
		t,
		app,
		http.MethodPut,
		"/api/v1/roles/"+role.ID+"/permissions",
		ownerLogin.Token,
		map[string]any{"permission_keys": []string{identity.PermissionHistoryReadOthers}},
		fiber.StatusOK,
	)
	request[[]sharedhistory.EntryView](
		t,
		app,
		http.MethodGet,
		"/api/v1/profiles/"+owner.ID+"/history?workspace_id="+workspace.ID,
		memberLogin.Token,
		nil,
		fiber.StatusOK,
	)

	privateWorkspace := request[workspaces.WorkspaceView](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces",
		ownerLogin.Token,
		map[string]any{"name": "Private history"},
		fiber.StatusCreated,
	).Data
	scopeDenied := request[[]sharedhistory.EntryView](
		t,
		app,
		http.MethodGet,
		"/api/v1/profiles/"+owner.ID+"/history?workspace_id="+privateWorkspace.ID,
		memberLogin.Token,
		nil,
		fiber.StatusForbidden,
	)
	if scopeDenied.Error.Code != "workspace_access_denied" {
		t.Fatalf("private workspace error = %+v, want workspace_access_denied", scopeDenied.Error)
	}

	request[map[string]bool](
		t,
		app,
		http.MethodDelete,
		"/api/v1/workspaces/"+workspace.ID+"/history",
		memberLogin.Token,
		nil,
		fiber.StatusOK,
	)
	cleared := request[[]sharedhistory.EntryView](
		t,
		app,
		http.MethodGet,
		"/api/v1/profiles/"+member.ID+"/history?workspace_id="+workspace.ID,
		memberLogin.Token,
		nil,
		fiber.StatusOK,
	).Data
	if len(cleared) != 0 {
		t.Fatalf("history after clear = %+v, want empty", cleared)
	}
}

func TestChangeAndAuditLogsExposeDiffsAndEnforceAuditPermission(t *testing.T) {
	app, usersService, _, closeDatabase := newTestServer(t)
	t.Cleanup(closeDatabase)

	owner, err := usersService.BootstrapOwner(t.Context(), users.CreateInput{
		Email: "log-owner", DisplayName: "Log Owner", Password: ownerPassword,
	})
	if err != nil {
		t.Fatalf("bootstrap owner: %v", err)
	}
	ownerLogin := login(t, app, owner.Email, ownerPassword)
	workspace := request[workspaces.WorkspaceView](
		t, app, http.MethodPost, "/api/v1/workspaces", ownerLogin.Token,
		map[string]any{"name": "Before workspace"}, fiber.StatusCreated,
	).Data
	request[workspaces.WorkspaceView](
		t, app, http.MethodPatch, "/api/v1/workspaces/"+workspace.ID, ownerLogin.Token,
		map[string]any{"name": "After workspace"}, fiber.StatusOK,
	)

	changeLog := request[activitylog.PageView](
		t, app, http.MethodGet,
		"/api/v1/workspaces/"+workspace.ID+"/change-log",
		ownerLogin.Token, nil, fiber.StatusOK,
	).Data.Entries
	var workspaceRename *activitylog.EntryView
	for index := range changeLog {
		entry := &changeLog[index]
		if entry.Resource == string(resourceevents.ResourceWorkspace) &&
			entry.Action == string(resourceevents.ActionUpdated) {
			workspaceRename = entry
			break
		}
	}
	if workspaceRename == nil || workspaceRename.ActorUserID != owner.ID ||
		workspaceRename.ActorDisplayName != owner.DisplayName ||
		!hasActivityDiff(workspaceRename.Diffs, "name", "Before workspace", "After workspace") {
		t.Fatalf("workspace rename log = %+v", workspaceRename)
	}
	firstPage := request[activitylog.PageView](
		t, app, http.MethodGet,
		"/api/v1/workspaces/"+workspace.ID+"/change-log?limit=1",
		ownerLogin.Token, nil, fiber.StatusOK,
	).Data
	if len(firstPage.Entries) != 1 || firstPage.OlderCursor == nil || firstPage.NewerCursor == nil {
		t.Fatalf("first cursor page = %+v", firstPage)
	}
	olderPage := request[activitylog.PageView](
		t, app, http.MethodGet,
		"/api/v1/workspaces/"+workspace.ID+"/change-log?limit=1&cursor="+*firstPage.OlderCursor,
		ownerLogin.Token, nil, fiber.StatusOK,
	).Data
	if len(olderPage.Entries) != 1 || olderPage.Entries[0].ID == firstPage.Entries[0].ID {
		t.Fatalf("older cursor page = %+v", olderPage)
	}
	request[workspaces.WorkspaceView](
		t, app, http.MethodPatch, "/api/v1/workspaces/"+workspace.ID, ownerLogin.Token,
		map[string]any{"name": "Newest workspace"}, fiber.StatusOK,
	)
	newerPage := request[activitylog.PageView](
		t, app, http.MethodGet,
		"/api/v1/workspaces/"+workspace.ID+"/change-log?limit=1&after="+*firstPage.NewerCursor,
		ownerLogin.Token, nil, fiber.StatusOK,
	).Data
	if len(newerPage.Entries) != 1 || newerPage.NewerCursor == nil ||
		!hasActivityDiff(newerPage.Entries[0].Diffs, "name", "After workspace", "Newest workspace") {
		t.Fatalf("newer cursor page = %+v", newerPage)
	}

	memberPassword := "audit-member-password"
	member := request[identity.UserView](
		t, app, http.MethodPost, "/api/v1/users", ownerLogin.Token,
		map[string]any{
			"email": "audit-member", "display_name": "Audit Member",
			"password": memberPassword, "role_ids": []string{},
		},
		fiber.StatusCreated,
	).Data
	request[identity.UserView](
		t, app, http.MethodPatch, "/api/v1/users/"+member.ID, ownerLogin.Token,
		map[string]any{"display_name": "Renamed Member", "password": "new-audit-member-password"},
		fiber.StatusOK,
	)
	role := request[identity.RoleView](
		t, app, http.MethodPost, "/api/v1/roles", ownerLogin.Token,
		map[string]any{
			"name": "Audit role", "description": "Before", "permission_keys": []string{},
		},
		fiber.StatusCreated,
	).Data
	request[identity.RoleView](
		t, app, http.MethodPatch, "/api/v1/roles/"+role.ID, ownerLogin.Token,
		map[string]any{"description": "After"}, fiber.StatusOK,
	)

	auditLog := request[activitylog.PageView](
		t, app, http.MethodGet, "/api/v1/audit-log", ownerLogin.Token, nil, fiber.StatusOK,
	).Data.Entries
	var sawUserDiff, sawRoleDiff, sawProtectedPassword bool
	for _, entry := range auditLog {
		if entry.Resource == string(resourceevents.ResourceUser) && entry.ResourceID == member.ID {
			sawUserDiff = sawUserDiff || hasActivityDiff(entry.Diffs, "display_name", "Audit Member", "Renamed Member")
			sawProtectedPassword = sawProtectedPassword || hasActivityDiff(entry.Diffs, "password", "[REDACTED]", "[CHANGED]")
		}
		if entry.Resource == string(resourceevents.ResourceRole) && entry.ResourceID == role.ID {
			sawRoleDiff = sawRoleDiff || hasActivityDiff(entry.Diffs, "description", "Before", "After")
		}
	}
	if !sawUserDiff || !sawRoleDiff || !sawProtectedPassword {
		t.Fatalf(
			"audit diffs user=%v role=%v password=%v log=%+v",
			sawUserDiff, sawRoleDiff, sawProtectedPassword, auditLog,
		)
	}
	serializedAudit, err := json.Marshal(auditLog)
	if err != nil {
		t.Fatalf("marshal audit log: %v", err)
	}
	if strings.Contains(string(serializedAudit), memberPassword) ||
		strings.Contains(string(serializedAudit), "new-audit-member-password") {
		t.Fatalf("password leaked into audit log: %s", serializedAudit)
	}

	readerRole := request[identity.RoleView](
		t, app, http.MethodPost, "/api/v1/roles", ownerLogin.Token,
		map[string]any{
			"name": "Change log reader", "description": "Resource scoped",
			"permission_keys": []string{
				identity.PermissionWorkspacesRead,
				identity.PermissionCollectionsRead,
				identity.PermissionRequestsRead,
			},
		},
		fiber.StatusCreated,
	).Data
	request[identity.UserView](
		t, app, http.MethodPut, "/api/v1/users/"+member.ID+"/roles", ownerLogin.Token,
		map[string]any{"role_ids": []string{readerRole.ID}}, fiber.StatusOK,
	)
	navigationParent := request[workspaces.CollectionView](
		t, app, http.MethodPost, "/api/v1/workspaces/"+workspace.ID+"/collections", ownerLogin.Token,
		map[string]any{"name": "Navigation parent"}, fiber.StatusCreated,
	).Data
	visibleCollection := request[workspaces.CollectionView](
		t, app, http.MethodPost, "/api/v1/workspaces/"+workspace.ID+"/collections", ownerLogin.Token,
		map[string]any{
			"name": "Visible collection", "parent_collection_id": navigationParent.ID,
		}, fiber.StatusCreated,
	).Data
	hiddenCollection := request[workspaces.CollectionView](
		t, app, http.MethodPost, "/api/v1/workspaces/"+workspace.ID+"/collections", ownerLogin.Token,
		map[string]any{"name": "Hidden collection"}, fiber.StatusCreated,
	).Data
	request[workspaces.CollectionView](
		t, app, http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+visibleCollection.ID+"/users",
		ownerLogin.Token, map[string]any{"user_ids": []string{member.ID}}, fiber.StatusOK,
	)
	request[workspaces.CollectionView](
		t, app, http.MethodPatch,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+visibleCollection.ID,
		ownerLogin.Token, map[string]any{"name": "Visible after"}, fiber.StatusOK,
	)
	request[workspaces.CollectionView](
		t, app, http.MethodPatch,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+hiddenCollection.ID,
		ownerLogin.Token, map[string]any{"name": "Hidden after"}, fiber.StatusOK,
	)
	request[workspaces.CollectionView](
		t, app, http.MethodPatch,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+navigationParent.ID,
		ownerLogin.Token, map[string]any{"name": "Navigation after"}, fiber.StatusOK,
	)

	memberLogin := login(t, app, member.Email, "new-audit-member-password")
	memberChangeLog := request[activitylog.PageView](
		t, app, http.MethodGet, "/api/v1/workspaces/"+workspace.ID+"/change-log",
		memberLogin.Token, nil, fiber.StatusOK,
	).Data.Entries
	var sawVisibleCollection bool
	for _, entry := range memberChangeLog {
		if entry.CollectionID == hiddenCollection.ID || entry.CollectionID == navigationParent.ID ||
			entry.Resource == string(resourceevents.ResourceWorkspace) {
			t.Fatalf("collection-scoped change log exposed out-of-scope entry: %+v", entry)
		}
		sawVisibleCollection = sawVisibleCollection ||
			entry.CollectionID == visibleCollection.ID &&
				hasActivityDiff(entry.Diffs, "name", "Visible collection", "Visible after")
	}
	if !sawVisibleCollection {
		t.Fatalf("collection-scoped change log omitted visible rename: %+v", memberChangeLog)
	}

	denied := request[activitylog.PageView](
		t, app, http.MethodGet, "/api/v1/audit-log", memberLogin.Token, nil, fiber.StatusForbidden,
	)
	if denied.Error.Code != "forbidden" {
		t.Fatalf("audit permission error = %+v", denied.Error)
	}
}

func hasActivityDiff(diffs []activitylog.DiffView, field string, from, to any) bool {
	for _, diff := range diffs {
		if diff.Field == field && reflect.DeepEqual(diff.From, from) && reflect.DeepEqual(diff.To, to) {
			return true
		}
	}
	return false
}

func TestWorkspaceAndRecursiveCollectionScopes(t *testing.T) {
	app, usersService, _, closeDatabase := newTestServer(t)
	defer closeDatabase()

	owner, err := usersService.BootstrapOwner(t.Context(), users.CreateInput{
		Email:       "owner",
		DisplayName: "Owner",
		Password:    ownerPassword,
	})
	if err != nil {
		t.Fatalf("bootstrap owner: %v", err)
	}
	ownerLogin := login(t, app, owner.Email, ownerPassword)

	resourcePermissions := []string{
		identity.PermissionWorkspacesRead,
		identity.PermissionWorkspacesCreate,
		identity.PermissionWorkspacesUpdate,
		identity.PermissionWorkspacesDelete,
		identity.PermissionWorkspacesAssignUsers,
		identity.PermissionCollectionsRead,
		identity.PermissionCollectionsCreate,
		identity.PermissionCollectionsUpdate,
		identity.PermissionCollectionsDelete,
		identity.PermissionCollectionsAssignUsers,
		identity.PermissionRequestsRead,
		identity.PermissionRequestsCreate,
		identity.PermissionRequestsUpdate,
		identity.PermissionRequestsDelete,
	}
	role := request[identity.RoleView](t, app, http.MethodPost, "/api/v1/roles", ownerLogin.Token, map[string]any{
		"name":            "Workspace collaborator",
		"description":     "Exercises resource scopes",
		"permission_keys": resourcePermissions,
	}, fiber.StatusCreated).Data

	createCollaborator := func(login string) identity.UserView {
		t.Helper()
		return request[identity.UserView](t, app, http.MethodPost, "/api/v1/users", ownerLogin.Token, map[string]any{
			"email":        login,
			"display_name": login,
			"password":     collaboratorPassword,
			"role_ids":     []string{role.ID},
		}, fiber.StatusCreated).Data
	}
	broadUser := createCollaborator("broad")
	nestedUser := createCollaborator("nested")
	outsiderUser := createCollaborator("outsider")
	broadLogin := login(t, app, broadUser.Email, collaboratorPassword)
	nestedLogin := login(t, app, nestedUser.Email, collaboratorPassword)
	outsiderLogin := login(t, app, outsiderUser.Email, collaboratorPassword)

	workspace := request[workspaces.WorkspaceView](t, app, http.MethodPost, "/api/v1/workspaces", ownerLogin.Token, map[string]any{
		"name": "Team API",
	}, fiber.StatusCreated).Data
	if len(workspace.UserIDs) != 1 || workspace.UserIDs[0] != owner.ID {
		t.Fatalf("workspace creator grants = %v, want [%s]", workspace.UserIDs, owner.ID)
	}
	assertCreator(t, workspace.CreatedBy, owner.ID, owner.Email)

	createCollection := func(name string, parentID *string) workspaces.CollectionView {
		t.Helper()
		body := map[string]any{"name": name}
		if parentID != nil {
			body["parent_collection_id"] = *parentID
		}
		return request[workspaces.CollectionView](
			t,
			app,
			http.MethodPost,
			"/api/v1/workspaces/"+workspace.ID+"/collections",
			ownerLogin.Token,
			body,
			fiber.StatusCreated,
		).Data
	}
	product := createCollection("Product", nil)
	assertCreator(t, product.CreatedBy, owner.ID, owner.Email)
	admin := createCollection("Admin", &product.ID)
	secrets := createCollection("Secrets", &admin.ID)
	public := createCollection("Public", &product.ID)
	other := createCollection("Other", nil)
	createSavedRequest := func(collectionID, name, url string) workspaces.SavedRequestView {
		t.Helper()
		return request[workspaces.SavedRequestView](
			t,
			app,
			http.MethodPost,
			"/api/v1/workspaces/"+workspace.ID+"/collections/"+collectionID+"/requests",
			ownerLogin.Token,
			map[string]any{
				"name": name,
				"definition": map[string]any{
					"request": map[string]any{"method": "GET", "url": url},
					"scripts": map[string]any{},
				},
			},
			fiber.StatusCreated,
		).Data
	}
	productRequest := createSavedRequest(product.ID, "Product request", "https://example.com/product")
	assertCreator(t, productRequest.CreatedBy, owner.ID, owner.Email)
	adminRequest := createSavedRequest(admin.ID, "Admin request", "https://example.com/admin")
	secretsRequest := createSavedRequest(secrets.ID, "Secrets request", "https://example.com/secrets")
	createSavedRequest(public.ID, "Public request", "https://example.com/public")

	workspace = request[workspaces.WorkspaceView](
		t,
		app,
		http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/users",
		ownerLogin.Token,
		map[string]any{"user_ids": []string{owner.ID, broadUser.ID}},
		fiber.StatusOK,
	).Data
	if !containsString(workspace.UserIDs, broadUser.ID) {
		t.Fatalf("workspace grants = %v, missing broad user", workspace.UserIDs)
	}

	admin = request[workspaces.CollectionView](
		t,
		app,
		http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+admin.ID+"/users",
		ownerLogin.Token,
		map[string]any{"user_ids": []string{nestedUser.ID}},
		fiber.StatusOK,
	).Data
	if len(admin.UserIDs) != 1 || admin.UserIDs[0] != nestedUser.ID {
		t.Fatalf("admin grants = %v, want nested user", admin.UserIDs)
	}

	outsiderWorkspaces := request[[]workspaces.WorkspaceView](
		t, app, http.MethodGet, "/api/v1/workspaces", outsiderLogin.Token, nil, fiber.StatusOK,
	).Data
	if len(outsiderWorkspaces) != 0 {
		t.Fatalf("outsider workspaces = %v, want none", outsiderWorkspaces)
	}

	nestedWorkspaces := request[[]workspaces.WorkspaceView](
		t, app, http.MethodGet, "/api/v1/workspaces", nestedLogin.Token, nil, fiber.StatusOK,
	).Data
	if len(nestedWorkspaces) != 1 {
		t.Fatalf("nested workspace count = %d, want 1", len(nestedWorkspaces))
	}
	nestedWorkspace := nestedWorkspaces[0]
	if len(nestedWorkspace.UserIDs) != 0 {
		t.Fatalf("collection-scoped workspace users = %v, want hidden", nestedWorkspace.UserIDs)
	}
	if nestedWorkspace.CreatedBy != nil {
		t.Fatalf("collection-scoped workspace creator = %+v, want hidden", nestedWorkspace.CreatedBy)
	}
	if len(nestedWorkspace.Collections) != 1 || nestedWorkspace.Collections[0].ID != product.ID {
		t.Fatalf("nested roots = %+v, want Product ancestor shell", nestedWorkspace.Collections)
	}
	productShell := nestedWorkspace.Collections[0]
	if len(productShell.UserIDs) != 0 {
		t.Fatalf("ancestor shell users = %v, want hidden", productShell.UserIDs)
	}
	if productShell.CreatedBy != nil {
		t.Fatalf("ancestor shell creator = %+v, want hidden", productShell.CreatedBy)
	}
	if len(productShell.Requests) != 0 {
		t.Fatalf("ancestor shell requests = %+v, want hidden", productShell.Requests)
	}
	if len(productShell.SubCollections) != 1 || productShell.SubCollections[0].ID != admin.ID {
		t.Fatalf("visible Product children = %+v, want only Admin", productShell.SubCollections)
	}
	visibleAdmin := productShell.SubCollections[0]
	if len(visibleAdmin.Requests) != 1 || visibleAdmin.Requests[0].ID != adminRequest.ID {
		t.Fatalf("visible admin requests = %+v, want Admin request", visibleAdmin.Requests)
	}
	if len(visibleAdmin.SubCollections) != 1 || visibleAdmin.SubCollections[0].ID != secrets.ID {
		t.Fatalf("Admin descendants = %+v, want Secrets", visibleAdmin.SubCollections)
	}
	if len(visibleAdmin.SubCollections[0].Requests) != 1 || visibleAdmin.SubCollections[0].Requests[0].ID != secretsRequest.ID {
		t.Fatalf("visible secret requests = %+v, want Secrets request", visibleAdmin.SubCollections[0].Requests)
	}
	if visibleAdmin.SubCollections[0].ID == public.ID || productShell.ID == other.ID {
		t.Fatal("collection-scoped tree exposed an inaccessible sibling")
	}

	ancestorDenied := request[workspaces.CollectionView](
		t,
		app,
		http.MethodGet,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+product.ID,
		nestedLogin.Token,
		nil,
		fiber.StatusForbidden,
	)
	if ancestorDenied.Error.Code != "collection_access_denied" {
		t.Fatalf("ancestor access code = %q", ancestorDenied.Error.Code)
	}
	request[workspaces.SavedRequestView](
		t,
		app,
		http.MethodGet,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+product.ID+"/requests/"+productRequest.ID,
		nestedLogin.Token,
		nil,
		fiber.StatusForbidden,
	)
	updatedRequest := request[workspaces.SavedRequestView](
		t,
		app,
		http.MethodPatch,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+admin.ID+"/requests/"+adminRequest.ID,
		nestedLogin.Token,
		map[string]any{
			"name": "Updated admin request",
			"definition": map[string]any{
				"request": map[string]any{"method": "POST", "url": "https://example.com/updated"},
				"scripts": map[string]any{},
			},
		},
		fiber.StatusOK,
	).Data
	if updatedRequest.Name != "Updated admin request" || !bytes.Contains(updatedRequest.Definition, []byte(`"method":"POST"`)) {
		t.Fatalf("updated request = %+v", updatedRequest)
	}
	moveDenied := request[workspaces.SavedRequestView](
		t,
		app,
		http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+admin.ID+"/requests/"+adminRequest.ID+"/collection",
		nestedLogin.Token,
		map[string]any{"target_collection_id": other.ID},
		fiber.StatusForbidden,
	)
	if moveDenied.Error.Code != "collection_access_denied" {
		t.Fatalf("request move access code = %q", moveDenied.Error.Code)
	}
	movedRequest := request[workspaces.SavedRequestView](
		t,
		app,
		http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+product.ID+"/requests/"+productRequest.ID+"/collection",
		ownerLogin.Token,
		map[string]any{"target_collection_id": other.ID},
		fiber.StatusOK,
	).Data
	if movedRequest.ID != productRequest.ID || movedRequest.CollectionID != other.ID {
		t.Fatalf("moved request = %+v, want request %s in collection %s", movedRequest, productRequest.ID, other.ID)
	}
	request[workspaces.SavedRequestView](
		t,
		app,
		http.MethodGet,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+product.ID+"/requests/"+productRequest.ID,
		ownerLogin.Token,
		nil,
		fiber.StatusNotFound,
	)
	request[workspaces.CollectionView](
		t,
		app,
		http.MethodGet,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+secrets.ID,
		nestedLogin.Token,
		nil,
		fiber.StatusOK,
	)

	nestedChild := request[workspaces.CollectionView](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+workspace.ID+"/collections",
		nestedLogin.Token,
		map[string]any{"name": "Nested child", "parent_collection_id": admin.ID},
		fiber.StatusCreated,
	).Data
	if nestedChild.ParentCollectionID == nil || *nestedChild.ParentCollectionID != admin.ID {
		t.Fatalf("nested child parent = %v, want %s", nestedChild.ParentCollectionID, admin.ID)
	}
	assertCreator(t, nestedChild.CreatedBy, nestedUser.ID, nestedUser.Email)

	rootDenied := request[workspaces.CollectionView](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+workspace.ID+"/collections",
		nestedLogin.Token,
		map[string]any{"name": "Unauthorized root"},
		fiber.StatusForbidden,
	)
	if rootDenied.Error.Code != "workspace_access_denied" {
		t.Fatalf("root creation code = %q", rootDenied.Error.Code)
	}

	workspaceUpdateDenied := request[workspaces.WorkspaceView](
		t,
		app,
		http.MethodPatch,
		"/api/v1/workspaces/"+workspace.ID,
		nestedLogin.Token,
		map[string]any{"name": "Unauthorized rename"},
		fiber.StatusForbidden,
	)
	if workspaceUpdateDenied.Error.Code != "workspace_access_denied" {
		t.Fatalf("workspace update code = %q", workspaceUpdateDenied.Error.Code)
	}

	request[workspaces.CollectionView](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+workspace.ID+"/collections",
		broadLogin.Token,
		map[string]any{"name": "Broad root"},
		fiber.StatusCreated,
	)

	cycle := request[workspaces.CollectionView](
		t,
		app,
		http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+product.ID+"/parent",
		ownerLogin.Token,
		map[string]any{"parent_collection_id": secrets.ID},
		fiber.StatusConflict,
	)
	if cycle.Error.Code != "collection_cycle" {
		t.Fatalf("cycle code = %q, want collection_cycle", cycle.Error.Code)
	}

	movedToRoot := request[workspaces.CollectionView](
		t,
		app,
		http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+public.ID+"/parent",
		ownerLogin.Token,
		map[string]any{"parent_collection_id": nil},
		fiber.StatusOK,
	).Data
	if movedToRoot.ParentCollectionID != nil {
		t.Fatalf("moved root parent = %v, want nil", movedToRoot.ParentCollectionID)
	}
	movedBack := request[workspaces.CollectionView](
		t,
		app,
		http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+public.ID+"/parent",
		ownerLogin.Token,
		map[string]any{"parent_collection_id": product.ID},
		fiber.StatusOK,
	).Data
	if movedBack.ParentCollectionID == nil || *movedBack.ParentCollectionID != product.ID {
		t.Fatalf("moved child parent = %v, want %s", movedBack.ParentCollectionID, product.ID)
	}

	moveToRootDenied := request[workspaces.CollectionView](
		t,
		app,
		http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+admin.ID+"/parent",
		nestedLogin.Token,
		map[string]any{"parent_collection_id": nil},
		fiber.StatusForbidden,
	)
	if moveToRootDenied.Error.Code != "workspace_access_denied" {
		t.Fatalf("move-to-root code = %q", moveToRootDenied.Error.Code)
	}

	unknownUser := request[workspaces.CollectionView](
		t,
		app,
		http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+admin.ID+"/users",
		ownerLogin.Token,
		map[string]any{"user_ids": []string{uuid.NewString()}},
		fiber.StatusUnprocessableEntity,
	)
	if unknownUser.Error.Fields["user_ids"] == "" {
		t.Fatalf("unknown-user response = %+v", unknownUser.Error)
	}

	request[struct{}](
		t,
		app,
		http.MethodDelete,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+admin.ID,
		ownerLogin.Token,
		nil,
		fiber.StatusOK,
	)
	request[workspaces.CollectionView](
		t,
		app,
		http.MethodGet,
		"/api/v1/workspaces/"+workspace.ID+"/collections/"+secrets.ID,
		ownerLogin.Token,
		nil,
		fiber.StatusNotFound,
	)
	request[struct{}](
		t,
		app,
		http.MethodDelete,
		"/api/v1/workspaces/"+workspace.ID,
		ownerLogin.Token,
		nil,
		fiber.StatusOK,
	)
	request[workspaces.WorkspaceView](
		t,
		app,
		http.MethodGet,
		"/api/v1/workspaces/"+workspace.ID,
		ownerLogin.Token,
		nil,
		fiber.StatusNotFound,
	)
}

func TestEnvironmentValuesAreEncryptedUserScopedAndPreservedAcrossPasswordChange(t *testing.T) {
	app, usersService, db, closeDatabase := newTestServer(t)
	defer closeDatabase()

	owner, err := usersService.BootstrapOwner(t.Context(), users.CreateInput{
		Email:       "owner",
		DisplayName: "Owner",
		Password:    ownerPassword,
	})
	if err != nil {
		t.Fatalf("bootstrap owner: %v", err)
	}
	ownerLogin := login(t, app, owner.Email, ownerPassword)
	workspace := request[[]workspaces.WorkspaceView](
		t, app, http.MethodGet, "/api/v1/workspaces", ownerLogin.Token, nil, fiber.StatusOK,
	).Data[0]

	role := request[identity.RoleView](t, app, http.MethodPost, "/api/v1/roles", ownerLogin.Token, map[string]any{
		"name":        "Environment user",
		"description": "Reads shared keys and writes personal values",
		"permission_keys": []string{
			identity.PermissionEnvironmentsRead,
			identity.PermissionEnvironmentValuesUpdate,
		},
	}, fiber.StatusCreated).Data
	createUser := func(loginName string) identity.UserView {
		t.Helper()
		return request[identity.UserView](t, app, http.MethodPost, "/api/v1/users", ownerLogin.Token, map[string]any{
			"email":        loginName,
			"display_name": loginName,
			"password":     collaboratorPassword,
			"role_ids":     []string{role.ID},
		}, fiber.StatusCreated).Data
	}
	alice := createUser("alice")
	bob := createUser("bob")
	request[workspaces.WorkspaceView](
		t,
		app,
		http.MethodPut,
		"/api/v1/workspaces/"+workspace.ID+"/users",
		ownerLogin.Token,
		map[string]any{"user_ids": []string{owner.ID, alice.ID, bob.ID}},
		fiber.StatusOK,
	)
	aliceLogin := login(t, app, alice.Email, collaboratorPassword)
	bobLogin := login(t, app, bob.Email, collaboratorPassword)

	environment := request[workspaces.EnvironmentView](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+workspace.ID+"/environments",
		ownerLogin.Token,
		map[string]any{"name": "Production"},
		fiber.StatusCreated,
	).Data
	variable := request[workspaces.EnvironmentVariableView](
		t,
		app,
		http.MethodPost,
		"/api/v1/workspaces/"+workspace.ID+"/environments/"+environment.ID+"/variables",
		ownerLogin.Token,
		map[string]any{
			"key":    "api_token",
			"value":  "owner-token",
			"secret": true,
		},
		fiber.StatusCreated,
	).Data
	if variable.Value != "owner-token" || !variable.Enabled || !variable.Secret {
		t.Fatalf("created variable = %+v", variable)
	}

	sharedKey := "api_token"
	valueFor := func(token string) string {
		t.Helper()
		environments := request[[]workspaces.EnvironmentView](
			t,
			app,
			http.MethodGet,
			"/api/v1/workspaces/"+workspace.ID+"/environments",
			token,
			nil,
			fiber.StatusOK,
		).Data
		if len(environments) != 1 || len(environments[0].Variables) != 1 {
			t.Fatalf("environments = %+v, want one environment and key", environments)
		}
		if environments[0].Variables[0].ID != variable.ID || environments[0].Variables[0].Key != sharedKey {
			t.Fatalf("shared variable = %+v, want %s/%s", environments[0].Variables[0], variable.ID, sharedKey)
		}
		return environments[0].Variables[0].Value
	}
	if value := valueFor(aliceLogin.Token); value != "" {
		t.Fatalf("Alice initial value = %q, want empty", value)
	}
	if value := valueFor(bobLogin.Token); value != "" {
		t.Fatalf("Bob initial value = %q, want empty", value)
	}

	valuePath := "/api/v1/workspaces/" + workspace.ID + "/environments/" + environment.ID + "/variables/" + variable.ID + "/value"
	request[workspaces.EnvironmentVariableView](
		t, app, http.MethodPut, valuePath, aliceLogin.Token, map[string]any{"value": "alice-token"}, fiber.StatusOK,
	)
	request[workspaces.EnvironmentVariableView](
		t, app, http.MethodPut, valuePath, bobLogin.Token, map[string]any{"value": "bob-token"}, fiber.StatusOK,
	)
	if value := valueFor(ownerLogin.Token); value != "owner-token" {
		t.Fatalf("owner value = %q, want owner-token", value)
	}
	if value := valueFor(aliceLogin.Token); value != "alice-token" {
		t.Fatalf("Alice value = %q, want alice-token", value)
	}
	if value := valueFor(bobLogin.Token); value != "bob-token" {
		t.Fatalf("Bob value = %q, want bob-token", value)
	}

	sharedKey = "service_token"
	renamed := request[workspaces.EnvironmentVariableView](
		t,
		app,
		http.MethodPatch,
		"/api/v1/workspaces/"+workspace.ID+"/environments/"+environment.ID+"/variables/"+variable.ID,
		ownerLogin.Token,
		map[string]any{"key": sharedKey},
		fiber.StatusOK,
	).Data
	if renamed.Value != "owner-token" || !renamed.Enabled || !renamed.Secret {
		t.Fatalf("renamed variable = %+v, want owner value and unchanged flags", renamed)
	}
	if value := valueFor(aliceLogin.Token); value != "alice-token" {
		t.Fatalf("Alice value after shared key rename = %q, want alice-token", value)
	}

	columns, err := db.Migrator().ColumnTypes(&workspaces.EnvironmentVariable{})
	if err != nil {
		t.Fatalf("inspect environment variable columns: %v", err)
	}
	for _, column := range columns {
		if column.Name() == "value" {
			t.Fatal("global environment_variables table contains a value column")
		}
	}
	for _, forbidden := range []struct {
		model  any
		column string
	}{
		{model: &identity.User{}, column: "environment_key_salt"},
		{model: &identity.User{}, column: "environment_key_ciphertext"},
		{model: &identity.Session{}, column: "environment_key_ciphertext"},
	} {
		if db.Migrator().HasColumn(forbidden.model, forbidden.column) {
			t.Fatalf("database stores forbidden environment key column %s", forbidden.column)
		}
	}
	var storedValues []workspaces.EnvironmentVariableValue
	if err := db.Order("user_id ASC").Find(&storedValues).Error; err != nil {
		t.Fatalf("load encrypted values: %v", err)
	}
	if len(storedValues) != 3 {
		t.Fatalf("stored value count = %d, want 3", len(storedValues))
	}
	for _, stored := range storedValues {
		for _, plaintext := range []string{"owner-token", "alice-token", "bob-token"} {
			if bytes.Contains(stored.Ciphertext, []byte(plaintext)) {
				t.Fatalf("ciphertext for user %s contains plaintext %q", stored.UserID, plaintext)
			}
		}
	}
	var aliceStored workspaces.EnvironmentVariableValue
	if err := db.First(
		&aliceStored,
		"environment_variable_id = ? AND user_id = ?",
		variable.ID,
		alice.ID,
	).Error; err != nil {
		t.Fatalf("load Alice ciphertext: %v", err)
	}
	oldAliceCiphertext := append([]byte(nil), aliceStored.Ciphertext...)

	newAlicePassword := "alice changed password"
	request[identity.UserView](
		t,
		app,
		http.MethodPatch,
		"/api/v1/users/"+alice.ID,
		ownerLogin.Token,
		map[string]any{"password": newAlicePassword},
		fiber.StatusOK,
	)
	request[any](t, app, http.MethodGet, "/api/v1/workspaces/"+workspace.ID+"/environments", aliceLogin.Token, nil, fiber.StatusUnauthorized)
	aliceLogin = login(t, app, alice.Email, newAlicePassword)
	if value := valueFor(aliceLogin.Token); value != "alice-token" {
		t.Fatalf("Alice value after password change = %q, want alice-token", value)
	}
	if err := db.First(
		&aliceStored,
		"environment_variable_id = ? AND user_id = ?",
		variable.ID,
		alice.ID,
	).Error; err != nil {
		t.Fatalf("reload Alice ciphertext: %v", err)
	}
	if bytes.Equal(oldAliceCiphertext, aliceStored.Ciphertext) {
		t.Fatal("password change did not re-encrypt Alice's value")
	}

	request[struct{}](t, app, http.MethodPost, "/api/v1/auth/logout", bobLogin.Token, nil, fiber.StatusOK)
	newBobPassword := "bob changed password"
	rejectedReset := request[identity.UserView](
		t,
		app,
		http.MethodPatch,
		"/api/v1/users/"+bob.ID,
		ownerLogin.Token,
		map[string]any{"password": newBobPassword},
		fiber.StatusConflict,
	)
	if rejectedReset.Error.Code != "environment_key_unavailable" {
		t.Fatalf("password reset error = %+v, want environment_key_unavailable", rejectedReset.Error)
	}
	bobLogin = login(t, app, bob.Email, collaboratorPassword)
	if value := valueFor(bobLogin.Token); value != "bob-token" {
		t.Fatalf("Bob value after rejected password reset = %q, want bob-token", value)
	}
}

func assertCreator(t *testing.T, creator *identity.UserSummaryView, userID, login string) {
	t.Helper()
	if creator == nil || creator.ID != userID || creator.Email != login {
		t.Fatalf("creator = %+v, want user %s (%s)", creator, userID, login)
	}
}

func newTestServer(t *testing.T) (*fiber.App, *users.Service, *gorm.DB, func()) {
	t.Helper()
	dsn := fmt.Sprintf("file:%s?mode=memory&cache=shared&_foreign_keys=on", uuid.NewString())
	db, err := database.Open(config.Database{Driver: "sqlite", DSN: dsn})
	if err != nil {
		t.Fatalf("open test database: %v", err)
	}
	sqlDatabase, err := db.DB()
	if err != nil {
		t.Fatalf("access test database: %v", err)
	}
	if err := database.MigrateAndSeed(db); err != nil {
		_ = sqlDatabase.Close()
		t.Fatalf("migrate test database: %v", err)
	}
	dataKeyProvider, err := security.NewStaticKeyProvider(
		"server-test-v1",
		base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0x45}, security.DataKeyLength)),
	)
	if err != nil {
		_ = sqlDatabase.Close()
		t.Fatalf("create data key provider: %v", err)
	}
	dataCipher, err := security.NewDataCipher(dataKeyProvider)
	if err != nil {
		_ = sqlDatabase.Close()
		t.Fatalf("create data cipher: %v", err)
	}

	repository := identity.NewRepository(db, dataCipher)
	if err := repository.EncryptLegacyRoles(t.Context()); err != nil {
		dataCipher.Close()
		_ = sqlDatabase.Close()
		t.Fatalf("encrypt seeded roles: %v", err)
	}
	passwordParams := security.PasswordParams{
		Memory:      8 * 1024,
		Iterations:  1,
		Parallelism: 1,
		SaltLength:  16,
		KeyLength:   32,
	}
	hasher := security.NewPasswordHasher(passwordParams)
	environmentCipher, err := security.NewEnvironmentCipher(
		"test deployment encryption secret with enough bytes",
		passwordParams,
	)
	if err != nil {
		_ = sqlDatabase.Close()
		t.Fatalf("create environment cipher: %v", err)
	}
	sessionKeys := security.NewSessionEnvironmentKeys()
	authService, err := auth.NewService(repository, hasher, environmentCipher, sessionKeys, time.Hour)
	if err != nil {
		_ = sqlDatabase.Close()
		t.Fatalf("create auth service: %v", err)
	}
	events := eventsystem.NewEventListener()
	events.StartNewEventLoop()
	activityRepository := activitylog.NewRepository(db, dataCipher)
	recordedEvents := activitylog.NewRecorder(activityRepository, events)
	usersService := users.NewService(
		repository,
		hasher,
		environmentCipher,
		sessionKeys,
		users.WithFirstOwnerSetup(func(ctx context.Context, tx *gorm.DB, owner identity.User) error {
			return workspaces.SetupFirstOwnerWorkspaceEncrypted(ctx, tx, owner, dataCipher)
		}),
		users.WithPasswordChangeSetup(func(
			ctx context.Context,
			tx *gorm.DB,
			userID string,
			oldKey, newKey []byte,
		) error {
			return workspaces.RekeyEnvironmentVariableValues(ctx, tx, environmentCipher, userID, oldKey, newKey)
		}),
		users.WithEvents(recordedEvents),
	)
	rolesService := roles.NewService(repository, roles.WithEvents(recordedEvents))
	workspaceRepository := workspaces.NewRepository(db, dataCipher)
	workspacesService := workspaces.NewService(
		workspaceRepository,
		environmentCipher,
		workspaces.WithEvents(recordedEvents),
	)
	realtimePublisher := realtime.New(events)
	requestProxyHandler := requestproxy.NewHandler(requestproxy.NewService(
		workspacesService,
		requestproxy.NewSettingsRepository(db, dataCipher),
	))
	sharedHistoryHandler := sharedhistory.NewHandler(sharedhistory.NewService(
		sharedhistory.NewRepository(db, dataCipher),
		workspacesService,
		sharedhistory.WithEvents(events),
	))
	activityHandler := activitylog.NewHandler(activitylog.NewService(
		activityRepository,
		workspacesService,
	))
	httpServer := server.New(
		"127.0.0.1:0",
		io.Discard,
		server.WithIdentity(
			authService,
			auth.NewHandler(authService),
			users.NewHandler(usersService),
			roles.NewHandler(rolesService),
		),
		server.WithWorkspaces(authService, workspaces.NewHandler(workspacesService)),
		server.WithRequestProxy(authService, requestProxyHandler),
		server.WithSharedHistory(authService, sharedHistoryHandler),
		server.WithActivityLogs(authService, activityHandler),
		server.WithRealtime(authService, realtimePublisher),
	)
	return httpServer.App, usersService, db, func() {
		events.Stop()
		dataCipher.Close()
		_ = sqlDatabase.Close()
	}
}

func containsString(values []string, expected string) bool {
	for _, value := range values {
		if value == expected {
			return true
		}
	}
	return false
}

func login(t *testing.T, app *fiber.App, email, password string) auth.LoginResponse {
	t.Helper()
	response := request[auth.LoginResponse](t, app, http.MethodPost, "/api/v1/auth/login", "", map[string]any{
		"email":    email,
		"password": password,
	}, fiber.StatusOK)
	if response.Data.Token == "" {
		t.Fatal("login response did not include a token")
	}
	return response.Data
}

func request[Data any](
	t *testing.T,
	app *fiber.App,
	method, path, token string,
	body any,
	wantStatus int,
) apiResponse[Data] {
	t.Helper()
	var encoded []byte
	var err error
	if body != nil {
		encoded, err = json.Marshal(body)
		if err != nil {
			t.Fatalf("encode request body: %v", err)
		}
	}
	httpRequest, err := http.NewRequest(method, "http://resolved.test"+path, bytes.NewReader(encoded))
	if err != nil {
		t.Fatalf("create request: %v", err)
	}
	if body != nil {
		httpRequest.Header.Set("Content-Type", "application/json")
	}
	if token != "" {
		httpRequest.Header.Set("Authorization", "Bearer "+token)
	}

	httpResponse, err := app.Test(httpRequest)
	if err != nil {
		t.Fatalf("execute %s %s: %v", method, path, err)
	}
	defer httpResponse.Body.Close()
	responseBody, err := io.ReadAll(httpResponse.Body)
	if err != nil {
		t.Fatalf("read response: %v", err)
	}
	if httpResponse.StatusCode != wantStatus {
		t.Fatalf("%s %s status = %d, want %d; body=%s", method, path, httpResponse.StatusCode, wantStatus, responseBody)
	}
	var response apiResponse[Data]
	if err := json.Unmarshal(responseBody, &response); err != nil {
		t.Fatalf("decode response %s: %v", responseBody, err)
	}
	if wantStatus < 400 && !response.Success {
		t.Fatalf("%s %s returned unsuccessful response: %s", method, path, responseBody)
	}
	if wantStatus >= 400 && response.Success {
		t.Fatalf("%s %s returned successful error response: %s", method, path, responseBody)
	}
	return response
}
