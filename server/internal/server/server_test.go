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
	"resolved-server/internal/workspaces"

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
	initialWorkspaces := request[[]workspaces.WorkspaceView](
		t, app, http.MethodGet, "/api/v1/workspaces", ownerLogin.Token, nil, fiber.StatusOK,
	).Data
	if len(initialWorkspaces) != 1 || initialWorkspaces[0].Name != workspaces.DefaultWorkspaceName {
		t.Fatalf("initial workspaces = %+v, want one %q workspace", initialWorkspaces, workspaces.DefaultWorkspaceName)
	}
	if len(initialWorkspaces[0].UserIDs) != 1 || initialWorkspaces[0].UserIDs[0] != owner.ID {
		t.Fatalf("initial workspace users = %v, want [%s]", initialWorkspaces[0].UserIDs, owner.ID)
	}
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

func TestWorkspaceAndRecursiveCollectionScopes(t *testing.T) {
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
	admin := createCollection("Admin", &product.ID)
	secrets := createCollection("Secrets", &admin.ID)
	public := createCollection("Public", &product.ID)
	other := createCollection("Other", nil)

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
	if len(nestedWorkspace.Collections) != 1 || nestedWorkspace.Collections[0].ID != product.ID {
		t.Fatalf("nested roots = %+v, want Product ancestor shell", nestedWorkspace.Collections)
	}
	productShell := nestedWorkspace.Collections[0]
	if len(productShell.UserIDs) != 0 {
		t.Fatalf("ancestor shell users = %v, want hidden", productShell.UserIDs)
	}
	if len(productShell.SubCollections) != 1 || productShell.SubCollections[0].ID != admin.ID {
		t.Fatalf("visible Product children = %+v, want only Admin", productShell.SubCollections)
	}
	visibleAdmin := productShell.SubCollections[0]
	if len(visibleAdmin.SubCollections) != 1 || visibleAdmin.SubCollections[0].ID != secrets.ID {
		t.Fatalf("Admin descendants = %+v, want Secrets", visibleAdmin.SubCollections)
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
	usersService := users.NewService(
		repository,
		hasher,
		users.WithFirstOwnerSetup(workspaces.SetupFirstOwnerWorkspace),
	)
	rolesService := roles.NewService(repository)
	workspaceRepository := workspaces.NewRepository(db)
	workspacesService := workspaces.NewService(workspaceRepository)
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
	)
	return httpServer.App, usersService, func() { _ = sqlDatabase.Close() }
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
