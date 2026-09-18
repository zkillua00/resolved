package requestproxy

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"syscall"
	"testing"

	"github.com/gofiber/fiber/v3"
	"github.com/gofiber/fiber/v3/middleware/requestid"
	"github.com/gorilla/websocket"
	"resolved-server/internal/executionlimits"
	"resolved-server/internal/httpkit"
	"resolved-server/internal/problem"
)

func TestExecutionBindingDiagnostics(t *testing.T) {
	app := fiber.New(fiber.Config{ErrorHandler: httpkit.ErrorHandler})
	app.Use(requestid.New())
	app.Post("/:workspace_id", func(c fiber.Ctx) error {
		c.SetContext(context.WithValue(c.Context(), executionSnapshotKey{}, executionSnapshot{snapshot: testSnapshot()}))
		return httpkit.WithProcessedPayload[any, ExecutePayload, ExecuteRequest](func(c fiber.Ctx, p ExecutePayload) httpkit.Response[any] {
			return httpkit.NewSuccessResponse[any](200, nil)
		})(c)
	})
	for _, tc := range []struct {
		body, field, reason string
		status              int
	}{
		{`{"method":12345,"url":"https://secret:password@example.com"}`, "method", "type_mismatch", 400},
		{`null`, "body", "type_mismatch", 400},
		{`{"url":`, "body", "invalid_json", 400},
		{`{"url":"https://example.com"}`, "method", "invalid_field", 422},
		{`{"method":"GET","url":"https://example.com","collection_id":"secret"}`, "collection_id", "invalid_field", 422},
	} {
		req := httptest.NewRequest("POST", "/workspace", strings.NewReader(tc.body))
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("X-Request-ID", "diagnostic-id")
		resp, err := app.Test(req)
		if err != nil {
			t.Fatal(err)
		}
		var body httpkit.Envelope[any]
		err = json.NewDecoder(resp.Body).Decode(&body)
		resp.Body.Close()
		if err != nil || resp.StatusCode != tc.status || body.Error == nil || body.Error.Fields[tc.field] == "" || body.Error.Reason != tc.reason || body.RequestID != "diagnostic-id" {
			t.Fatalf("body %s: status=%d envelope=%+v error=%+v decode=%v", tc.body, resp.StatusCode, body, body.Error, err)
		}
		encoded, _ := json.Marshal(body)
		if tc.field == "method" && tc.reason == "type_mismatch" && body.Error.Fields["method"] != "expected string; received number" {
			t.Fatalf("missing expected/received detail: %+v", body.Error)
		}
		if strings.Contains(string(encoded), "secret") || strings.Contains(string(encoded), "12345") {
			t.Fatal(string(encoded))
		}
	}
	req := httptest.NewRequest("POST", "/workspace", strings.NewReader("secret-invalid-gzip"))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Content-Encoding", "gzip")
	resp, err := app.Test(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var body httpkit.Envelope[any]
	if err := json.NewDecoder(resp.Body).Decode(&body); err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 400 || body.Error == nil || body.Error.Phase != "binding" || body.Error.Reason != "invalid_content_encoding" {
		t.Fatalf("lost decompression cause: status=%d error=%+v", resp.StatusCode, body.Error)
	}
	if !strings.Contains(body.Error.Fields["body"], "Content-Encoding") || strings.Contains(body.Error.Message, "secret") {
		t.Fatalf("unsafe or unhelpful decompression diagnostic: %+v", body.Error)
	}
}

func TestWebSocketOpeningDiagnostics(t *testing.T) {
	snapshot := testSnapshot()
	snapshot.Effective["websocket.opening_bytes"] = executionlimits.Bound{Value: 4096}
	snapshot.Effective["websocket.handshake_timeout_ms"] = executionlimits.Bound{Value: 1000}
	h := NewHandler(nil)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		ctx := context.WithValue(r.Context(), websocketExecutionContextKey{}, websocketExecutionContext{snapshot: snapshot, requestID: "ws-id"})
		h.handleWebSocket(w, r.WithContext(ctx))
	}))
	defer server.Close()
	for _, tc := range []struct {
		kind                   int
		payload, field, reason string
	}{
		{websocket.BinaryMessage, "secret", "", "unexpected_frame"},
		{websocket.TextMessage, `{"url":12345}`, "url", "type_mismatch"},
		{websocket.TextMessage, `{`, "body", "invalid_json"},
		{websocket.TextMessage, `{}`, "url", "invalid_field"},
		{websocket.TextMessage, `{"url":"ws://example.com","collection_id":"secret"}`, "collection_id", "invalid_field"},
	} {
		conn, _, err := websocket.DefaultDialer.Dial("ws"+strings.TrimPrefix(server.URL, "http"), nil)
		if err != nil {
			t.Fatal(err)
		}
		if err := conn.WriteMessage(tc.kind, []byte(tc.payload)); err != nil {
			t.Fatal(err)
		}
		var result websocketOpenResponse
		err = conn.ReadJSON(&result)
		conn.Close()
		if err != nil || result.Type != "error" || result.RequestID != "ws-id" || result.Error == nil || result.Error.Reason != tc.reason || result.Message != result.Error.Message {
			t.Fatalf("result=%+v error=%+v read=%v", result, result.Error, err)
		}
		if tc.field != "" && result.Error.Fields[tc.field] == "" {
			t.Fatalf("missing field: %+v", result)
		}
	}
	resp, err := http.Get(server.URL)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var envelope httpkit.Envelope[any]
	if err := json.NewDecoder(resp.Body).Decode(&envelope); err != nil {
		t.Fatal(err)
	}
	if envelope.RequestID != "ws-id" || envelope.Error.Reason != "invalid_upgrade" {
		t.Fatalf("%+v", envelope)
	}
}

func TestTransportDiagnosticsDoNotEchoErrors(t *testing.T) {
	err := proxyTransportError(&url.Error{Op: "Get", URL: "https://secret:password@example.com?token=secret", Err: errors.New("secret payload")})
	body := httpkit.NewErrorResponse[any](err)
	encoded, _ := json.Marshal(body)
	if strings.Contains(string(encoded), "secret") || body.Error.Reason != "connection_failed" {
		t.Fatal(string(encoded))
	}
	internal := httpkit.NewErrorResponse[any](problem.Wrap(errors.New("secret"), "private"))
	encoded, _ = json.Marshal(internal)
	if strings.Contains(string(encoded), "secret") || strings.Contains(string(encoded), "private") {
		t.Fatal(string(encoded))
	}
	for _, tc := range []struct {
		cause  error
		reason string
	}{
		{&net.DNSError{Name: "secret", Err: "private"}, "dns_failed"},
		{&tls.CertificateVerificationError{Err: errors.New("secret")}, "tls_certificate_invalid"},
		{context.DeadlineExceeded, "timeout"},
	} {
		response := httpkit.NewErrorResponse[any](proxyTransportError(tc.cause))
		encoded, _ := json.Marshal(response)
		if response.Error.Reason != tc.reason || strings.Contains(string(encoded), "secret") || strings.Contains(string(encoded), "private") {
			t.Fatal(string(encoded))
		}
	}
}

func TestBlockedRequestPreservesAllowlistAction(t *testing.T) {
	request := "https://secret:password@example.com/?token=secret#secret"
	response := httpkit.NewErrorResponse[any](proxyTransportError(&blockedDestinationError{
		Request: request, Address: "127.0.0.1", Reason: "loopback",
	}))
	if response.Error.Fields["request"] != request || response.Error.Reason != "destination_blocked" {
		t.Fatalf("lost exact allowlist action: %+v", response.Error)
	}
	if strings.Contains(response.Error.Message, "secret") {
		t.Fatal("request leaked into summary")
	}
}

func TestTypedTransportDiagnostics(t *testing.T) {
	for _, tc := range []struct {
		cause  error
		reason string
	}{
		{syscall.ECONNREFUSED, "connection_refused"},
		{syscall.ECONNRESET, "connection_reset"},
		{syscall.ECONNABORTED, "connection_aborted"},
		{io.EOF, "eof"},
		{io.ErrUnexpectedEOF, "unexpected_eof"},
		{context.Canceled, "canceled"},
		{&net.DNSError{Name: "secret", Err: "secret", IsNotFound: true}, "dns_not_found"},
		{&net.DNSError{Name: "secret", Err: "secret", IsTemporary: true}, "dns_temporary"},
		{x509.UnknownAuthorityError{}, "tls_untrusted"},
		{x509.HostnameError{Host: "secret"}, "tls_hostname_mismatch"},
		{&tls.CertificateVerificationError{Err: x509.CertificateInvalidError{Reason: x509.Expired, Detail: "secret"}}, "tls_certificate_expired"},
		{x509.CertificateInvalidError{Reason: x509.IncompatibleUsage, Detail: "secret"}, "tls_certificate_invalid"},
	} {
		t.Run(tc.reason, func(t *testing.T) {
			response := httpkit.NewErrorResponse[any](proxyTransportError(&url.Error{
				URL: "https://secret", Err: &net.OpError{Op: "read", Err: tc.cause},
			}))
			encoded, _ := json.Marshal(response)
			if response.Error.Reason != tc.reason || response.Error.Fields["operation"] != "read" || strings.Contains(string(encoded), "secret") {
				t.Fatal(string(encoded))
			}
		})
	}
	response := httpkit.NewErrorResponse[any](proxyTransportError(&net.OpError{Op: "secret", Err: io.EOF}))
	if len(response.Error.Fields) != 0 {
		t.Fatalf("arbitrary operation exposed: %+v", response.Error)
	}
}

func TestDescriptorDiagnosticDetails(t *testing.T) {
	for _, tc := range []struct{ input, field, detail string }{
		{`[]`, "body", "expected a JSON object; received array"},
		{`"secret"`, "body", "expected a JSON object; received string"},
		{`true`, "body", "expected a JSON object; received boolean"},
		{`12345`, "body", "expected a JSON object; received number"},
		{`null`, "body", "expected a JSON object; received null"},
		{`  {"url":`, "body", "expected a valid JSON object; received malformed JSON at byte offset 9"},
		{`{"headers":[{"value":12345}]}`, "headers.value", "expected string; received number"},
		{`{"body":{"fields":[{"filename":12345}]}}`, "body.fields.filename", "expected string; received number"},
	} {
		var input ExecuteRequest
		response := httpkit.NewErrorResponse[any](decodeExecutionDescriptor([]byte(tc.input), &input, "binding"))
		if response.Error.Fields[tc.field] != tc.detail {
			t.Fatalf("%s: %+v", tc.input, response.Error)
		}
	}
	var input struct {
		Values map[string]string `json:"values"`
	}
	response := httpkit.NewErrorResponse[any](decodeExecutionDescriptor([]byte(`{"values":{"secret":12345}}`), &input, "binding"))
	encoded, _ := json.Marshal(response)
	if strings.Contains(string(encoded), "secret") || strings.Contains(string(encoded), "12345") || response.Error.Fields["values"] == "" {
		t.Fatal(string(encoded))
	}
}

func TestHeaderDiagnosticsIdentifyPosition(t *testing.T) {
	for _, tc := range []struct {
		header Header
		field  string
	}{
		{Header{Name: "secret bad name", Value: "secret"}, "headers[2].name"},
		{Header{Name: "X-Secret", Value: "secret\r\ninjected"}, "headers[2].value"},
		{Header{Name: "X-Secret", Value: "secret\x00"}, "headers[2].value"},
	} {
		request, _ := http.NewRequest("GET", "https://example.com", nil)
		response := httpkit.NewErrorResponse[any](applyHeaders(request, []Header{{Name: "Accept"}, {Name: "X-Test"}, tc.header}))
		encoded, _ := json.Marshal(response)
		if response.Error.Fields[tc.field] == "" || strings.Contains(string(encoded), "secret") {
			t.Fatal(string(encoded))
		}
	}
}

func TestWebSocketHTTPInternalErrorIsLogged(t *testing.T) {
	var logs bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewJSONHandler(&logs, nil)))
	defer slog.SetDefault(previous)
	recorder := httptest.NewRecorder()
	writeWebSocketHTTPError(recorder, "upgrade-id", problem.Wrap(errors.New("secret"), "private"))
	if !strings.Contains(logs.String(), `"request_id":"upgrade-id"`) || !strings.Contains(logs.String(), "secret") {
		t.Fatal("internal error was not logged with request ID")
	}
	if strings.Contains(recorder.Body.String(), "secret") || !strings.Contains(recorder.Body.String(), `"phase":"websocket_opening"`) {
		t.Fatal(recorder.Body.String())
	}
}

func TestWebSocketServiceErrorEnvelope(t *testing.T) {
	var logs bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewJSONHandler(&logs, nil)))
	defer slog.SetDefault(previous)
	for _, cause := range []error{
		invalidField("url", "must use ws:// or wss://"),
		problem.WithFields("proxy_destination_blocked", "destination blocked", map[string]string{"reason": "loopback"}),
		problem.Wrap(errors.New("secret credential"), "private diagnostic"),
	} {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			conn, err := (&websocket.Upgrader{}).Upgrade(w, r, nil)
			if err != nil {
				return
			}
			defer conn.Close()
			writeWebSocketOpenError(conn, cause, "ws-id")
		}))
		conn, _, err := websocket.DefaultDialer.Dial("ws"+strings.TrimPrefix(server.URL, "http"), nil)
		if err != nil {
			server.Close()
			t.Fatal(err)
		}
		var result websocketOpenResponse
		err = conn.ReadJSON(&result)
		conn.Close()
		server.Close()
		expected := httpkit.NewErrorResponse[any](cause).Error
		if err != nil || result.Error == nil || result.Error.Code != expected.Code || result.RequestID != "ws-id" || result.Error.Phase == "" || result.Error.Reason == "" {
			t.Fatalf("result=%+v err=%v", result, err)
		}
		for field, value := range expected.Fields {
			if result.Error.Fields[field] != value {
				t.Fatalf("lost field %s: %+v", field, result)
			}
		}
		encoded, _ := json.Marshal(result)
		if strings.Contains(string(encoded), "secret") || strings.Contains(string(encoded), "private") {
			t.Fatal(string(encoded))
		}
	}
	if !strings.Contains(logs.String(), `"request_id":"ws-id"`) || !strings.Contains(logs.String(), "secret credential") {
		t.Fatal("internal WebSocket error was not logged with request ID")
	}
}
