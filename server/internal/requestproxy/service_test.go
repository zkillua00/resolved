package requestproxy

import (
	"bytes"
	"context"
	"encoding/base64"
	"errors"
	"io"
	"mime"
	"mime/multipart"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"resolved-server/internal/problem"
	"resolved-server/internal/requestproxy/proxybody"
)

func TestBuildBodyPreservesOrderedFormFields(t *testing.T) {
	body, contentType, err := proxybody.Build(proxybody.Body{
		Mode: "form_url_encoded",
		Fields: []proxybody.BodyField{
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
	body, contentType, err := proxybody.Build(proxybody.Body{
		Mode: "multipart_form_data",
		Fields: []proxybody.BodyField{
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
	decoded, err := proxybody.Decode(base64.StdEncoding.EncodeToString([]byte("x")), 1)
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
			{Hostname: "API.Internal.", Target: "HTTPS://Gateway.Internal./"},
			{Hostname: "files.internal", Target: "[2001:0db8::1]"},
		},
	})
	if err != nil {
		t.Fatalf("normalize settings: %v", err)
	}
	if settings.Mode != ModeServer || len(settings.HostnameOverrides) != 2 {
		t.Fatalf("normalized settings = %+v", settings)
	}
	if settings.HostnameOverrides[0] != (HostnameOverride{Hostname: "api.internal", Target: "https://gateway.internal"}) {
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
				{Hostname: "api.internal", Target: "ftp://gateway.internal"},
			},
		},
		{
			Mode: ModeServer,
			HostnameOverrides: []HostnameOverride{
				{Hostname: "api.internal", Target: "https://gateway.internal:8443"},
			},
		},
		{
			Mode: ModeServer,
			HostnameOverrides: []HostnameOverride{
				{Hostname: "api.internal", Target: "https://gateway.internal/path"},
			},
		},
	}
	for index, settings := range tests {
		if _, err := normalizeSettings(settings); err == nil {
			t.Fatalf("case %d accepted invalid settings: %+v", index, settings)
		}
	}
}

func TestResolveRequestURLUsesSchemeFromExactOverride(t *testing.T) {
	overrides := map[string]hostnameOverrideTarget{
		"alias.internal": {Host: "gateway.internal", Scheme: "https"},
	}
	target, err := resolveRequestURL("alias.internal:8443/items?active=true", overrides)
	if err != nil {
		t.Fatalf("resolve scheme-less URL: %v", err)
	}
	if target.Scheme != "https" || target.Host != "alias.internal:8443" || target.RequestURI() != "/items?active=true" {
		t.Fatalf("resolved request URL = %s", target.String())
	}

	request := &http.Request{URL: target, Header: make(http.Header)}
	request = request.WithContext(context.WithValue(
		context.Background(),
		hostnameOverridesContextKey{},
		overrides,
	))
	applyHostnameOriginOverride(request)
	if request.URL.String() != "https://gateway.internal:8443/items?active=true" {
		t.Fatalf("overridden request URL = %s", request.URL)
	}
	if request.Host != "gateway.internal:8443" {
		t.Fatalf("overridden HTTP Host = %q", request.Host)
	}
}

func TestResolveRequestURLRejectsMissingSchemeWithoutSchemeAwareOverride(t *testing.T) {
	_, err := resolveRequestURL(
		"alias.internal/items",
		map[string]hostnameOverrideTarget{
			"alias.internal": {Host: "gateway.internal"},
		},
	)
	if err == nil {
		t.Fatal("scheme-less URL was accepted without an override scheme")
	}
	var validationError *problem.Error
	if !errors.As(err, &validationError) {
		t.Fatalf("validation error type = %T", err)
	}
	if validationError.Fields["url"] != "must include http:// or https:// unless its hostname override supplies a scheme" {
		t.Fatalf("URL validation reason = %q", validationError.Fields["url"])
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
		map[string]hostnameOverrideTarget{"example.com": {Host: "127.0.0.1"}},
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
		map[string]hostnameOverrideTarget{"alias.internal": {Host: "example.com"}},
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
		map[string]hostnameOverrideTarget{"alias.internal": {Host: "127.0.0.1"}},
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

func TestBlockedNetworkTargetsAreRejected(t *testing.T) {
	transport := transportWithHostnameOverrides(&http.Transport{})
	client := &http.Client{Transport: transport}
	ctx := context.WithValue(context.Background(), requestTargetHostContextKey{}, "example.com")
	for _, target := range []string{
		"http://127.0.0.1/",
		"http://10.0.0.1/",
		"http://172.16.0.1/",
		"http://192.168.1.1/",
		"http://169.254.169.254/",
		"http://100.64.0.1/",
		"http://[::1]/",
	} {
		request, err := http.NewRequestWithContext(ctx, http.MethodGet, target, nil)
		if err != nil {
			t.Fatalf("create request for %s: %v", target, err)
		}
		if _, err := client.Do(request); err == nil {
			t.Fatalf("expected %s to be blocked through the request proxy", target)
		}
	}
}

func TestAdminOverrideStillReachesAPrivateTarget(t *testing.T) {
	// Loopback is blocked by default, but an explicit admin hostname override
	// remains the documented escape hatch for private-network targets. The
	// override is keyed by the requested hostname, so a request whose URL host
	// matches the override is trusted and dials the configured target.
	transport := transportWithHostnameOverrides(&http.Transport{})
	ctx := context.WithValue(
		context.Background(),
		hostnameOverridesContextKey{},
		map[string]hostnameOverrideTarget{"internal.example": {Host: "127.0.0.1"}},
	)
	ctx = context.WithValue(ctx, requestTargetHostContextKey{}, "internal.example")
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://internal.example/", nil)
	if err != nil {
		t.Fatalf("create request: %v", err)
	}
	// applyHostnameOriginOverride keeps the origin and marks proxy bypass for an
	// IP target; the request is trusted (not blocked) because its hostname has
	// an admin override.
	applyHostnameOriginOverride(request)
	proxy, err := transport.Proxy(request)
	if err != nil {
		t.Fatalf("resolve override proxy: %v", err)
	}
	if proxy != nil {
		t.Fatalf("overridden origin unexpectedly routed through a proxy")
	}
}

func TestBlockedIPReasonCoversPrivateAndLoopbackRanges(t *testing.T) {
	for _, address := range []string{
		"127.0.0.1", "10.0.0.1", "172.16.0.1", "192.168.1.1",
		"169.254.169.254", "100.64.0.1", "::1", "fe80::1", "fc00::1",
	} {
		if blockedIPReason(net.ParseIP(address)) == "" {
			t.Fatalf("expected %s to be blocked", address)
		}
	}
	for _, address := range []string{"93.184.216.34", "8.8.8.8", "2001:4860:4860::8888"} {
		if blockedIPReason(net.ParseIP(address)) != "" {
			t.Fatalf("did not expect %s to be blocked", address)
		}
	}
}
