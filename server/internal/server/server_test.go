package server_test

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"testing"
	"time"

	"resolved-server/internal/auth"
	"resolved-server/internal/config"
	"resolved-server/internal/database"
	"resolved-server/internal/identity"
	"resolved-server/internal/roles"
	"resolved-server/internal/security"
	"resolved-server/internal/server"
	"resolved-server/internal/users"

	"github.com/gofiber/fiber/v3"
	"github.com/google/uuid"
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
	app, usersService, closeDatabase := newTestServer(t)
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

	userResponse := request[identity.UserView](t, app, http.MethodPost, "/api/v1/users", ownerLogin.Token, map[string]any{
		"email":        "collaborator",
		"display_name": "Collaborator",
		"password":     collaboratorPassword,
		"role_ids":     []string{roleResponse.Data.ID},
	}, fiber.StatusCreated)
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

func newTestServer(t *testing.T) (*fiber.App, *users.Service, func()) {
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

	repository := identity.NewRepository(db)
	hasher := security.NewPasswordHasher(security.PasswordParams{
		Memory:      8 * 1024,
		Iterations:  1,
		Parallelism: 1,
		SaltLength:  16,
		KeyLength:   32,
	})
	authService, err := auth.NewService(repository, hasher, time.Hour)
	if err != nil {
		_ = sqlDatabase.Close()
		t.Fatalf("create auth service: %v", err)
	}
	usersService := users.NewService(repository, hasher)
	rolesService := roles.NewService(repository)
	httpServer := server.New(
		"127.0.0.1:0",
		io.Discard,
		server.WithIdentity(
			authService,
			auth.NewHandler(authService),
			users.NewHandler(usersService),
			roles.NewHandler(rolesService),
		),
	)
	return httpServer.App, usersService, func() { _ = sqlDatabase.Close() }
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
