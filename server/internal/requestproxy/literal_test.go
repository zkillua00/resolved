package requestproxy

import (
	"bytes"
	"context"
	"encoding/base64"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"testing"

	"resolved-server/internal/executionlimits"
	"resolved-server/internal/requestproxy/proxybody"
	"resolved-server/internal/workspaces"
)

// Exercise the existing authorized service, destination override, and raw body
// envelope together; already-encoded multipart must never be regenerated.
func TestLiteralExecutionBytesAndAuthoredMethodCompatibility(t *testing.T) {
	db, proxies, repository := newProxyTestRepositories(t)
	if err := db.AutoMigrate(&executionlimits.Record{}, &workspaces.WorkspaceUser{}, &workspaces.CollectionUser{}); err != nil {
		t.Fatal(err)
	}
	workspaceID, collectionID, requestID := createProxyScopeFixtures(t, db)
	type captured struct {
		method, contentType string
		body                []byte
		repeated            []string
	}
	received := make(chan captured, 4)
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, err := io.ReadAll(r.Body)
		if err != nil {
			t.Error(err)
		}
		received <- captured{r.Method, r.Header.Get("Content-Type"), body, r.Header.Values("X-Repeat")}
		w.WriteHeader(http.StatusNoContent)
	}))
	defer upstream.Close()
	target, _ := url.Parse(upstream.URL)
	if err := db.Create(&SettingsRecord{ID: SettingsRecordID, Mode: ModeServer}).Error; err != nil {
		t.Fatal(err)
	}
	proxy, err := proxies.Create(t.Context(), nil, "Literal fixture", []HostnameOverride{{Hostname: "literal.test", Target: target.Hostname()}})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := proxies.ReplaceAssignments(t.Context(), proxy.ID, []ProxyAssignment{{ScopeKind: ProxyScopeRequest, ScopeID: requestID}}); err != nil {
		t.Fatal(err)
	}
	service := NewService(workspaces.NewService(workspaces.NewRepository(db), nil), repository, proxies)
	policy, err := service.Policy(t.Context())
	if err != nil || !policy.LiteralMethod {
		t.Fatalf("literal method capability: policy=%+v err=%v", policy, err)
	}
	for _, tc := range []struct {
		literal                             bool
		method, expectedMethod, contentType string
		body                                []byte
	}{
		{true, "mIxEd", "mIxEd", "", []byte{0, 255, 128, 13, 10}},
		{true, "POST", "POST", "multipart/form-data; boundary=fixed", []byte("--fixed\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\n\x00\xff\x80\r\n--fixed--\r\n")},
		{false, " mIxEd ", "MIXED", "", []byte("editor")},
	} {
		ctx := context.WithValue(t.Context(), executionSnapshotKey{}, executionSnapshot{workspaceID, collectionID, testSnapshot()})
		input := ExecuteInput{
			LiteralMethod: tc.literal, RequestID: requestID, CollectionID: collectionID,
			Method: tc.method, URL: "http://literal.test:" + target.Port() + "/a%2Fb?x=%20&x=+&bare",
			Headers: []Header{{Name: "X-Repeat", Value: "one"}, {Name: "X-Repeat", Value: "two"}},
			Body:    proxybody.Body{Mode: "raw", DataBase64: base64.StdEncoding.EncodeToString(tc.body)},
		}
		if tc.contentType != "" {
			input.Headers = append(input.Headers, Header{Name: "Content-Type", Value: tc.contentType})
		}
		if _, err := service.Execute(ctx, workspaces.Actor{Owner: true}, workspaceID, input); err != nil {
			t.Fatal(err)
		}
		got := <-received
		if got.method != tc.expectedMethod || got.contentType != tc.contentType || !bytes.Equal(got.body, tc.body) {
			t.Fatalf("received %+v, want method=%q type=%q body=%x", got, tc.expectedMethod, tc.contentType, tc.body)
		}
		if len(got.repeated) != 2 || got.repeated[0] != "one" || got.repeated[1] != "two" {
			t.Fatalf("repeated headers: %v", got.repeated)
		}
		// A new body representation must not create a new permission bypass.
		if _, err := service.Execute(ctx, workspaces.Actor{}, workspaceID, input); err == nil {
			t.Fatal("unauthorized literal execution succeeded")
		}
		select {
		case <-received:
			t.Fatal("unauthorized execution reached destination")
		default:
		}
	}
}
