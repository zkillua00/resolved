package requestproxy

import (
	"bytes"
	"context"
	"encoding/base64"
	"io"
	"mime"
	"mime/multipart"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

func TestBuildBodyPreservesOrderedFormFields(t *testing.T) {
	body, contentType, err := buildBody(Body{
		Mode: "form_url_encoded",
		Fields: []BodyField{
			{Name: "z", Kind: "text", Value: "last first"},
			{Name: "a", Kind: "text", Value: "one&two"},
		},
	})
	if err != nil {
		t.Fatalf("build form body: %v", err)
	}
	if contentType != "application/x-www-form-urlencoded" {
		t.Fatalf("content type = %q", contentType)
	}
	if got := string(body); got != "z=last+first&a=one%26two" {
		t.Fatalf("form body = %q", got)
	}
}

func TestBuildBodyMaterializesMultipartTextAndFiles(t *testing.T) {
	body, contentType, err := buildBody(Body{
		Mode: "multipart_form_data",
		Fields: []BodyField{
			{Name: "description", Kind: "text", Value: "fixture"},
			{
				Name:          "document",
				Kind:          "file",
				Filename:      "payload.json",
				ContentBase64: base64.StdEncoding.EncodeToString([]byte(`{"ok":true}`)),
			},
		},
	})
	if err != nil {
		t.Fatalf("build multipart body: %v", err)
	}
	mediaType, parameters, err := mime.ParseMediaType(contentType)
	if err != nil {
		t.Fatalf("parse multipart content type: %v", err)
	}
	if mediaType != "multipart/form-data" {
		t.Fatalf("media type = %q", mediaType)
	}
	reader := multipart.NewReader(bytes.NewReader(body), parameters["boundary"])
	first, err := reader.NextPart()
	if err != nil {
		t.Fatalf("read text part: %v", err)
	}
	text, err := io.ReadAll(first)
	if err != nil {
		t.Fatalf("read text content: %v", err)
	}
	if first.FormName() != "description" || string(text) != "fixture" {
		t.Fatalf("text part = %q %q", first.FormName(), text)
	}
	second, err := reader.NextPart()
	if err != nil {
		t.Fatalf("read file part: %v", err)
	}
	file, err := io.ReadAll(second)
	if err != nil {
		t.Fatalf("read file content: %v", err)
	}
	if second.FormName() != "document" || second.FileName() != "payload.json" {
		t.Fatalf("file metadata = %q %q", second.FormName(), second.FileName())
	}
	if second.Header.Get("Content-Type") != "application/json" || string(file) != `{"ok":true}` {
		t.Fatalf("file part = %q %q", second.Header.Get("Content-Type"), file)
	}
}

func TestApplyHeadersKeepsTargetHeadersButDropsHopByHopState(t *testing.T) {
	request, err := http.NewRequest(http.MethodPost, "https://example.test", strings.NewReader("body"))
	if err != nil {
		t.Fatalf("create request: %v", err)
	}
	err = applyHeaders(request, []Header{
		{Name: "Host", Value: "virtual.example.test"},
		{Name: "X-Test", Value: "one"},
		{Name: "X-Test", Value: "two"},
		{Name: "Connection", Value: "close"},
		{Name: "Content-Length", Value: "999"},
	})
	if err != nil {
		t.Fatalf("apply headers: %v", err)
	}
	if request.Host != "virtual.example.test" {
		t.Fatalf("host = %q", request.Host)
	}
	if got := request.Header.Values("X-Test"); len(got) != 2 || got[0] != "one" || got[1] != "two" {
		t.Fatalf("repeated header values = %v", got)
	}
	if request.Header.Get("Connection") != "" || request.Header.Get("Content-Length") != "" {
		t.Fatalf("hop-by-hop headers were retained: %v", request.Header)
	}
}

func TestDecodeBodyAllowsPaddedPayloadAtLimit(t *testing.T) {
	decoded, err := decodeBody(base64.StdEncoding.EncodeToString([]byte("x")), 1)
	if err != nil {
		t.Fatalf("decode body at limit: %v", err)
	}
	if string(decoded) != "x" {
		t.Fatalf("decoded body = %q", decoded)
	}
}

func TestNormalizeSettingsCanonicalizesHostnamesAndIPTargets(t *testing.T) {
	settings, err := normalizeSettings(Settings{
		Mode: ModeServer,
		HostnameOverrides: []HostnameOverride{
			{Hostname: "API.Internal.", Target: "Gateway.Internal."},
			{Hostname: "files.internal", Target: "[2001:0db8::1]"},
		},
	})
	if err != nil {
		t.Fatalf("normalize settings: %v", err)
	}
	if settings.Mode != ModeServer || len(settings.HostnameOverrides) != 2 {
		t.Fatalf("normalized settings = %+v", settings)
	}
	if settings.HostnameOverrides[0] != (HostnameOverride{Hostname: "api.internal", Target: "gateway.internal"}) {
		t.Fatalf("hostname target = %+v", settings.HostnameOverrides[0])
	}
	if settings.HostnameOverrides[1] != (HostnameOverride{Hostname: "files.internal", Target: "2001:db8::1"}) {
		t.Fatalf("IP target = %+v", settings.HostnameOverrides[1])
	}
}

func TestNormalizeSettingsRejectsAmbiguousOverrides(t *testing.T) {
	tests := []Settings{
		{
			Mode: ModeServer,
			HostnameOverrides: []HostnameOverride{
				{Hostname: "api.internal", Target: "127.0.0.1"},
				{Hostname: "API.INTERNAL.", Target: "127.0.0.2"},
			},
		},
		{
			Mode: ModeServer,
			HostnameOverrides: []HostnameOverride{
				{Hostname: "127.0.0.1", Target: "gateway.internal"},
			},
		},
		{
			Mode: ModeServer,
			HostnameOverrides: []HostnameOverride{
				{Hostname: "api.internal", Target: "http://gateway.internal"},
			},
		},
	}
	for index, settings := range tests {
		if _, err := normalizeSettings(settings); err == nil {
			t.Fatalf("case %d accepted invalid settings: %+v", index, settings)
		}
	}
}

func TestIPAddressOverrideChangesDialTargetButPreservesHTTPAndTLSOrigin(t *testing.T) {
	type observedOrigin struct {
		host string
		sni  string
	}
	observed := make(chan observedOrigin, 1)
	target := httptest.NewTLSServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		observed <- observedOrigin{host: request.Host, sni: request.TLS.ServerName}
		writer.WriteHeader(http.StatusNoContent)
	}))
	defer target.Close()

	targetURL, err := url.Parse(target.URL)
	if err != nil {
		t.Fatalf("parse target URL: %v", err)
	}
	_, port, err := net.SplitHostPort(targetURL.Host)
	if err != nil {
		t.Fatalf("split target host: %v", err)
	}
	baseTransport := target.Client().Transport.(*http.Transport)
	client := &http.Client{Transport: transportWithHostnameOverrides(baseTransport)}
	ctx := context.WithValue(
		context.Background(),
		hostnameOverridesContextKey{},
		map[string]string{"example.com": "127.0.0.1"},
	)
	request, err := http.NewRequestWithContext(
		ctx,
		http.MethodGet,
		"https://example.com:"+port+"/resource",
		nil,
	)
	if err != nil {
		t.Fatalf("create overridden request: %v", err)
	}
	applyHostnameOriginOverride(request)
	response, err := client.Do(request)
	if err != nil {
		t.Fatalf("execute overridden request: %v", err)
	}
	response.Body.Close()

	origin := <-observed
	if origin.host != "example.com:"+port {
		t.Fatalf("HTTP Host = %q", origin.host)
	}
	if origin.sni != "example.com" {
		t.Fatalf("TLS server name = %q", origin.sni)
	}
}

func TestHostnameTargetBecomesTheHTTPAndTLSOrigin(t *testing.T) {
	type observedOrigin struct {
		host string
		sni  string
	}
	observed := make(chan observedOrigin, 1)
	target := httptest.NewTLSServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		observed <- observedOrigin{host: request.Host, sni: request.TLS.ServerName}
		writer.WriteHeader(http.StatusNoContent)
	}))
	defer target.Close()

	targetURL, err := url.Parse(target.URL)
	if err != nil {
		t.Fatalf("parse target URL: %v", err)
	}
	_, port, err := net.SplitHostPort(targetURL.Host)
	if err != nil {
		t.Fatalf("split target host: %v", err)
	}
	baseTransport := target.Client().Transport.(*http.Transport).Clone()
	dialer := &net.Dialer{}
	baseTransport.DialContext = func(ctx context.Context, network, _ string) (net.Conn, error) {
		return dialer.DialContext(ctx, network, targetURL.Host)
	}
	client := &http.Client{Transport: transportWithHostnameOverrides(baseTransport)}
	ctx := context.WithValue(
		context.Background(),
		hostnameOverridesContextKey{},
		map[string]string{"alias.internal": "example.com"},
	)
	request, err := http.NewRequestWithContext(
		ctx,
		http.MethodGet,
		"https://alias.internal:"+port+"/resource",
		nil,
	)
	if err != nil {
		t.Fatalf("create overridden request: %v", err)
	}
	request.Host = "explicit-host-must-not-win.internal"
	applyHostnameOriginOverride(request)
	response, err := client.Do(request)
	if err != nil {
		t.Fatalf("execute overridden request: %v", err)
	}
	response.Body.Close()

	origin := <-observed
	if origin.host != "example.com:"+port {
		t.Fatalf("HTTP Host = %q", origin.host)
	}
	if origin.sni != "example.com" {
		t.Fatalf("TLS server name = %q", origin.sni)
	}
	if response.Request.URL.Host != "example.com:"+port {
		t.Fatalf("final request origin = %q", response.Request.URL.Host)
	}
}

func TestProxyBypassIsScopedToTheOverriddenOrigin(t *testing.T) {
	proxyURL, err := url.Parse("http://proxy.internal:8080")
	if err != nil {
		t.Fatalf("parse proxy URL: %v", err)
	}
	transport := transportWithHostnameOverrides(&http.Transport{
		Proxy: func(*http.Request) (*url.URL, error) { return proxyURL, nil },
	})
	ctx := context.WithValue(
		context.Background(),
		hostnameOverridesContextKey{},
		map[string]string{"alias.internal": "127.0.0.1"},
	)
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://alias.internal/resource", nil)
	if err != nil {
		t.Fatalf("create overridden request: %v", err)
	}
	applyHostnameOriginOverride(request)
	proxy, err := transport.Proxy(request)
	if err != nil {
		t.Fatalf("resolve overridden proxy: %v", err)
	}
	if proxy != nil {
		t.Fatalf("overridden origin used proxy %s", proxy)
	}

	redirect := request.Clone(request.Context())
	redirect.URL, err = url.Parse("http://other.internal/resource")
	if err != nil {
		t.Fatalf("parse redirect URL: %v", err)
	}
	proxy, err = transport.Proxy(redirect)
	if err != nil {
		t.Fatalf("resolve redirect proxy: %v", err)
	}
	if proxy == nil || proxy.String() != proxyURL.String() {
		t.Fatalf("unmapped redirect proxy = %v", proxy)
	}
}
