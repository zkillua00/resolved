package security

import (
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"strings"
	"testing"
)

type roundTripFunc func(*http.Request) (*http.Response, error)

func (f roundTripFunc) RoundTrip(request *http.Request) (*http.Response, error) {
	return f(request)
}

func TestVaultTransitKeyProviderRoundTrip(t *testing.T) {
	provider, err := NewVaultTransitKeyProvider(
		"https://vault.example.test/base", "vault-token", "team-a", "transit", "resolved_server",
	)
	if err != nil {
		t.Fatalf("create Vault provider: %v", err)
	}
	provider.client.Transport = roundTripFunc(func(request *http.Request) (*http.Response, error) {
		if request.Header.Get("X-Vault-Token") != "vault-token" || request.Header.Get("X-Vault-Namespace") != "team-a" {
			t.Fatalf("Vault headers = %v", request.Header)
		}
		var body map[string]string
		if err := json.NewDecoder(request.Body).Decode(&body); err != nil {
			t.Fatalf("decode request body: %v", err)
		}
		var response string
		switch {
		case strings.HasSuffix(request.URL.Path, "/encrypt/resolved_server"):
			response = `{"data":{"ciphertext":"vault:v1:` + body["plaintext"] + `"}}`
		case strings.HasSuffix(request.URL.Path, "/decrypt/resolved_server"):
			encoded := strings.TrimPrefix(body["ciphertext"], "vault:v1:")
			response = `{"data":{"plaintext":"` + encoded + `"}}`
		default:
			t.Fatalf("unexpected Vault path %s", request.URL.Path)
		}
		if body["associated_data"] != base64.StdEncoding.EncodeToString([]byte("scope-aad")) {
			t.Fatalf("associated data = %q", body["associated_data"])
		}
		return &http.Response{
			StatusCode: http.StatusOK,
			Header:     make(http.Header),
			Body:       io.NopCloser(strings.NewReader(response)),
		}, nil
	})

	key := []byte("0123456789abcdef0123456789abcdef")
	keyID, wrapped, err := provider.WrapKey(t.Context(), key, []byte("scope-aad"))
	if err != nil {
		t.Fatalf("wrap key: %v", err)
	}
	if keyID != "vault:transit:resolved_server" || string(wrapped) == string(key) {
		t.Fatalf("wrapped key ID = %q, wrapped = %q", keyID, wrapped)
	}
	unwrapped, err := provider.UnwrapKey(t.Context(), keyID, wrapped, []byte("scope-aad"))
	if err != nil {
		t.Fatalf("unwrap key: %v", err)
	}
	if string(unwrapped) != string(key) {
		t.Fatalf("unwrapped key = %q", unwrapped)
	}
}

func TestVaultTransitKeyProviderRequiresTLSOffLoopback(t *testing.T) {
	if _, err := NewVaultTransitKeyProvider(
		"http://vault.example.test", "token", "", "transit", "resolved",
	); err == nil {
		t.Fatal("expected non-loopback HTTP Vault address to fail")
	}
	if _, err := NewVaultTransitKeyProvider(
		"http://127.0.0.1:8200", "token", "", "transit", "resolved",
	); err != nil {
		t.Fatalf("loopback HTTP Vault address failed: %v", err)
	}
}
