package server_test

import (
	"bytes"
	"fmt"
	"github.com/gofiber/fiber/v3"
	"net"
	"net/http"
	"net/http/httptest"
	"resolved-server/internal/identity"
	"resolved-server/internal/requestproxy"
	"resolved-server/internal/users"
	"resolved-server/internal/workspaces"
	"strings"
	"testing"
)

func TestCookieJarsArePrivateEncryptedVersionedAndRekeyed(t *testing.T) {
	app, usersService, db, closeDB := newTestServer(t)
	defer closeDB()
	owner, err := usersService.BootstrapOwner(t.Context(), users.CreateInput{Email: "owner", DisplayName: "Owner", Password: ownerPassword})
	if err != nil {
		t.Fatal(err)
	}
	ownerLogin := login(t, app, owner.Email, ownerPassword)
	workspace := request[[]workspaces.WorkspaceView](t, app, http.MethodGet, "/api/v1/workspaces", ownerLogin.Token, nil, fiber.StatusOK).Data[0]
	role := request[identity.RoleView](t, app, http.MethodPost, "/api/v1/roles", ownerLogin.Token, map[string]any{"name": "Cookie user", "permission_keys": []string{identity.PermissionWorkspacesRead}}, fiber.StatusCreated).Data
	alice := request[identity.UserView](t, app, http.MethodPost, "/api/v1/users", ownerLogin.Token, map[string]any{"email": "alice", "display_name": "Alice", "password": collaboratorPassword, "role_ids": []string{role.ID}}, fiber.StatusCreated).Data
	aliceLogin := login(t, app, alice.Email, collaboratorPassword)
	path := "/api/v1/workspaces/" + workspace.ID + "/cookie-jar"
	request[any](t, app, http.MethodGet, path, aliceLogin.Token, nil, fiber.StatusForbidden)
	request[workspaces.WorkspaceView](t, app, http.MethodPut, "/api/v1/workspaces/"+workspace.ID+"/users", ownerLogin.Token, map[string]any{"user_ids": []string{owner.ID, alice.ID}}, fiber.StatusOK)
	jar := request[workspaces.CookieJar](t, app, http.MethodGet, path, aliceLogin.Token, nil, fiber.StatusOK).Data
	jar.Cookies = []workspaces.JarCookie{{URL: "https://api.example.com/private/login", Cookie: "session=alice-private; Secure; HttpOnly; Max-Age=600"}}
	saved := request[workspaces.CookieJar](t, app, http.MethodPut, path, aliceLogin.Token, jar, fiber.StatusOK).Data
	if saved.Revision != 1 || len(saved.Cookies) != 1 {
		t.Fatal("jar did not save")
	}
	request[any](t, app, http.MethodPut, path, aliceLogin.Token, jar, fiber.StatusConflict)
	ownerJar := request[workspaces.CookieJar](t, app, http.MethodGet, path, ownerLogin.Token, nil, fiber.StatusOK).Data
	if len(ownerJar.Cookies) != 0 {
		t.Fatal("owner read another user's cookies")
	}
	var row workspaces.CookieJarRecord
	if err := db.Where("user_id = ?", alice.ID).First(&row).Error; err != nil {
		t.Fatal(err)
	}
	before := append([]byte{}, row.Ciphertext...)
	if bytes.Contains(before, []byte("alice-private")) {
		t.Fatal("cookie not encrypted")
	}
	request[identity.UserView](t, app, http.MethodPatch, "/api/v1/users/"+alice.ID, ownerLogin.Token, map[string]any{"password": "new alice password"}, fiber.StatusOK)
	request[any](t, app, http.MethodGet, path, aliceLogin.Token, nil, fiber.StatusUnauthorized)
	aliceLogin = login(t, app, alice.Email, "new alice password")
	reloaded := request[workspaces.CookieJar](t, app, http.MethodGet, path, aliceLogin.Token, nil, fiber.StatusOK).Data
	if len(reloaded.Cookies) != 1 || !strings.Contains(reloaded.Cookies[0].Cookie, "alice-private") {
		t.Fatal("password change lost cookie")
	}
	if err := db.Where("user_id = ?", alice.ID).First(&row).Error; err != nil {
		t.Fatal(err)
	}
	if bytes.Equal(before, row.Ciphertext) {
		t.Fatal("password change did not reencrypt jar")
	}
	request[workspaces.CookieJar](t, app, http.MethodPut, path, aliceLogin.Token, map[string]any{"reset": true, "enabled": false}, fiber.StatusOK)
	empty := request[workspaces.CookieJar](t, app, http.MethodGet, path, aliceLogin.Token, nil, fiber.StatusOK).Data
	if empty.Enabled || len(empty.Cookies) != 0 {
		t.Fatal("reset failed")
	}
}
func TestServerExecutionPersistsRedirectCookiesWithoutSharingClients(t *testing.T) {
	app, usersService, _, closeDB := newTestServer(t)
	defer closeDB()
	owner, err := usersService.BootstrapOwner(t.Context(), users.CreateInput{Email: "owner", DisplayName: "Owner", Password: ownerPassword})
	if err != nil {
		t.Fatal(err)
	}
	token := login(t, app, owner.Email, ownerPassword).Token
	workspace := request[[]workspaces.WorkspaceView](t, app, http.MethodGet, "/api/v1/workspaces", token, nil, fiber.StatusOK).Data[0]
	received := make(chan string, 8)
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		received <- r.Header.Get("Cookie")
		if r.URL.Path == "/login" {
			w.Header().Add("Set-Cookie", "session=redirect-secret; Path=/; HttpOnly")
			http.Redirect(w, r, "/done", http.StatusFound)
			return
		}
		w.WriteHeader(http.StatusOK)
	}))
	defer target.Close()
	port := target.Listener.Addr().(*net.TCPAddr).Port
	request[requestproxy.Settings](t, app, http.MethodPut, "/api/v1/request-execution/settings", token, map[string]any{"mode": "server"}, fiber.StatusOK)
	cookieProxy := request[requestproxy.Proxy](t, app, http.MethodPost, "/api/v1/proxies", token, map[string]any{"name": "Cookie routing", "rules": []map[string]string{{"hostname": "cookie.internal", "target": "127.0.0.1"}}}, fiber.StatusCreated).Data
	request[requestproxy.Proxy](t, app, http.MethodPut, "/api/v1/proxies/"+cookieProxy.ID+"/assignments", token, map[string]any{"assignments": []map[string]string{{"scope_kind": "server"}}}, fiber.StatusOK)
	execute := func(path string, headers []map[string]string, useJar bool) {
		request[requestproxy.ExecuteResult](t, app, http.MethodPost, "/api/v1/workspaces/"+workspace.ID+"/execute", token, map[string]any{"method": "GET", "url": fmt.Sprintf("http://cookie.internal:%d%s", port, path), "headers": headers, "body": map[string]any{"mode": "none"}, "use_cookie_jar": useJar}, fiber.StatusOK)
	}
	execute("/login", nil, true)
	if first, second := <-received, <-received; first != "" || second != "session=redirect-secret" {
		t.Fatalf("redirect cookie handling: %q / %q", first, second)
	}
	execute("/done", nil, true)
	if got := <-received; got != "session=redirect-secret" {
		t.Fatalf("not persisted: %q", got)
	}
	execute("/done", []map[string]string{{"name": "Cookie", "value": "manual=explicit"}}, true)
	if got := <-received; got != "manual=explicit" {
		t.Fatalf("explicit precedence: %q", got)
	}
	execute("/done", nil, false)
	if got := <-received; got != "" {
		t.Fatalf("shared client leaked cookies: %q", got)
	}
}
