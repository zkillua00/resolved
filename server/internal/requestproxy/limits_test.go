package requestproxy

import (
	"bytes"
	"context"
	"encoding/base64"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"resolved-server/internal/executionlimits"
	"resolved-server/internal/problem"
	"resolved-server/internal/requestproxy/proxybody"
	"resolved-server/internal/workspaces"
)

func testSnapshot() executionlimits.Snapshot {
	return executionlimits.Snapshot{Effective: map[string]executionlimits.Bound{
		"http.timeout_ms": {Value: 60000}, "http.connect_timeout_ms": {Value: 30000},
		"http.tls_handshake_timeout_ms": {Value: 10000}, "http.redirects": {Value: 10},
		"http.request_bytes": {Value: 64 << 20}, "http.response_bytes": {Value: 64 << 20},
		"http.envelope_bytes": {Value: 96 << 20}, "http.url_bytes": {Value: 16384},
		"http.header_count": {Value: 256},
	}}
}

func TestExecutionBoundSemantics(t *testing.T) {
	for _, tc := range []struct {
		name     string
		bound    executionlimits.Bound
		tooLarge bool
	}{
		{"zero", executionlimits.Bound{}, true},
		{"lower", executionlimits.Bound{Value: 2}, true},
		{"exact", executionlimits.Bound{Value: 3}, false},
		{"higher", executionlimits.Bound{Value: 4}, false},
		{"unlimited", executionlimits.Bound{Unlimited: true}, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			_, tooLarge, err := readLimited(strings.NewReader("abc"), tc.bound)
			if err != nil || tooLarge != tc.tooLarge {
				t.Fatalf("read: %v, %v", tooLarge, err)
			}
		})
	}
	if limitDuration(executionlimits.Bound{}) == 0 {
		t.Fatal("zero timeout disabled deadline")
	}
	if limitDuration(executionlimits.Bound{Unlimited: true}) != 0 {
		t.Fatal("unlimited timeout has deadline")
	}
}

func TestConnectBudgetIncludesValidatedDNSLookup(t *testing.T) {
	previous := net.DefaultResolver
	net.DefaultResolver = &net.Resolver{PreferGo: true, Dial: func(ctx context.Context, _, _ string) (net.Conn, error) {
		<-ctx.Done()
		return nil, ctx.Err()
	}}
	t.Cleanup(func() { net.DefaultResolver = previous })
	snapshot := testSnapshot()
	snapshot.Effective["http.connect_timeout_ms"] = executionlimits.Bound{Value: 5}
	transport := executionTransport(http.DefaultTransport.(*http.Transport), snapshot)
	defer transport.CloseIdleConnections()
	ctx, cancel := context.WithTimeout(t.Context(), 500*time.Millisecond)
	defer cancel()
	ctx = context.WithValue(ctx, requestTargetHostContextKey{}, "delayed.invalid")
	started := time.Now()
	_, err := transport.DialContext(ctx, "tcp", "delayed.invalid:80")
	if err == nil || time.Since(started) > 250*time.Millisecond {
		t.Fatalf("connect budget did not cover DNS: elapsed=%s err=%v", time.Since(started), err)
	}
}

func TestWebSocketAdmissionUsesScopeAndCurrentBound(t *testing.T) {
	handler := NewHandler(nil)
	release, ok := handler.admitWebSocket("workspace/collection-a", executionlimits.Bound{Value: 1})
	if !ok {
		t.Fatal("initial admission denied")
	}
	if _, ok := handler.admitWebSocket("workspace/collection-a", executionlimits.Bound{}); ok {
		t.Fatal("lowered bound admitted new session")
	}
	other, ok := handler.admitWebSocket("workspace/collection-b", executionlimits.Bound{Value: 1})
	if !ok {
		t.Fatal("unrelated scope blocked")
	}
	unlimited, ok := handler.admitWebSocket("workspace/collection-a", executionlimits.Bound{Unlimited: true})
	if !ok {
		t.Fatal("unlimited admission blocked")
	}
	unlimited()
	other()
	release()
	release()
	if len(handler.websocketSlots) != 0 {
		t.Fatalf("leaked admissions: %v", handler.websocketSlots)
	}
}

func TestDescriptorLimits(t *testing.T) {
	snapshot := testSnapshot()
	snapshot.Effective["http.url_bytes"] = executionlimits.Bound{Value: 3}
	if validateDescriptor(snapshot, "abcd", nil) == nil {
		t.Fatal("long URL accepted")
	}
	snapshot.Effective["http.url_bytes"] = executionlimits.Bound{Unlimited: true}
	snapshot.Effective["http.header_count"] = executionlimits.Bound{}
	if validateDescriptor(snapshot, "abcd", []Header{{Name: "X-Test"}}) == nil {
		t.Fatal("zero headers ignored")
	}
	snapshot.Effective["http.header_count"] = executionlimits.Bound{Unlimited: true}
	if err := validateDescriptor(snapshot, "abcd", []Header{{Name: "X-Test"}}); err != nil {
		t.Fatal(err)
	}
}

func TestExecuteUsesFrozenSnapshotAndTimeout(t *testing.T) {
	db, proxies, repository := newProxyTestRepositories(t)
	if err := db.AutoMigrate(&executionlimits.Record{}, &workspaces.WorkspaceUser{}, &workspaces.CollectionUser{}); err != nil {
		t.Fatal(err)
	}
	workspaceID, collectionID, requestID := createProxyScopeFixtures(t, db)
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/redirect-two" {
			http.Redirect(w, r, "/redirect-one", http.StatusFound)
			return
		}
		if r.URL.Path == "/redirect-one" {
			http.Redirect(w, r, "/", http.StatusFound)
			return
		}
		if r.URL.Path == "/slow" {
			time.Sleep(30 * time.Millisecond)
		}
		_, _ = w.Write([]byte("response"))
	}))
	defer upstream.Close()
	target, err := url.Parse(upstream.URL)
	if err != nil {
		t.Fatal(err)
	}
	// Resolve this saved request's proxy alongside its frozen collection limits.
	// Override targets never contain ports; the request URL owns the port.
	if err := db.Create(&SettingsRecord{ID: SettingsRecordID, Mode: ModeServer}).Error; err != nil {
		t.Fatal(err)
	}
	proxy, err := proxies.Create(t.Context(), nil, "Limits fixture", []HostnameOverride{{Hostname: "limit.test", Target: target.Hostname()}})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := proxies.ReplaceAssignments(t.Context(), proxy.ID, []ProxyAssignment{{ScopeKind: ProxyScopeRequest, ScopeID: requestID}}); err != nil {
		t.Fatal(err)
	}
	service := NewService(workspaces.NewService(workspaces.NewRepository(db), nil), repository, proxies)
	execute := func(snapshot executionlimits.Snapshot, path string) error {
		ctx := context.WithValue(t.Context(), executionSnapshotKey{}, executionSnapshot{workspaceID, collectionID, snapshot})
		_, err := service.Execute(ctx, workspaces.Actor{Owner: true}, workspaceID, ExecuteInput{
			RequestID: requestID, CollectionID: collectionID, Method: "POST", URL: "http://limit.test:" + target.Port() + path,
			Body: proxybody.Body{Mode: "raw", DataBase64: base64.StdEncoding.EncodeToString(bytes.Repeat([]byte("x"), 4))},
		})
		return err
	}
	assertCode := func(err error, code string) {
		t.Helper()
		var p *problem.Error
		if !errors.As(err, &p) || p.Code != code {
			t.Fatalf("error = %v, want %s", err, code)
		}
	}
	snapshot := testSnapshot()
	snapshot.Effective["http.request_bytes"] = executionlimits.Bound{Value: 3}
	assertCode(execute(snapshot, "/"), "proxy_request_too_large")
	snapshot.Effective["http.request_bytes"] = executionlimits.Bound{Unlimited: true}
	snapshot.Effective["http.response_bytes"] = executionlimits.Bound{}
	assertCode(execute(snapshot, "/"), "proxy_response_too_large")
	snapshot.Effective["http.response_bytes"] = executionlimits.Bound{Unlimited: true}
	if err := execute(snapshot, "/"); err != nil {
		t.Fatal(err)
	}
	snapshot.Effective["http.timeout_ms"] = executionlimits.Bound{Value: 1}
	assertCode(execute(snapshot, "/slow"), "proxy_timeout")
	snapshot.Effective["http.timeout_ms"] = executionlimits.Bound{Unlimited: true}
	if err := execute(snapshot, "/slow"); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		bound   executionlimits.Bound
		path    string
		success bool
	}{
		{executionlimits.Bound{}, "/redirect-one", false},
		{executionlimits.Bound{Value: 1}, "/redirect-one", true},
		{executionlimits.Bound{Value: 1}, "/redirect-two", false},
		{executionlimits.Bound{Value: 2}, "/redirect-two", true},
		{executionlimits.Bound{Unlimited: true}, "/redirect-two", true},
	} {
		snapshot.Effective["http.redirects"] = tc.bound
		if err := execute(snapshot, tc.path); (err == nil) != tc.success {
			t.Fatalf("redirects=%+v path=%s err=%v", tc.bound, tc.path, err)
		}
	}
}
