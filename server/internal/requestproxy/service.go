package requestproxy

import (
	"bytes"
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/textproto"
	"net/url"
	"strconv"
	"strings"
	"time"

	"resolved-server/internal/problem"
	"resolved-server/internal/requestproxy/proxybody"
	"resolved-server/internal/resourceevents"
	"resolved-server/internal/workspaces"

	"github.com/google/uuid"
)

const (
	MaxResponseBodyBytes = 64 * 1024 * 1024
	DefaultTimeout       = 60 * time.Second
	MaxRedirects         = 10
)

type Service struct {
	workspaces *workspaces.Service
	settings   *SettingsRepository
	client     *http.Client
	events     resourceevents.Emitter
}

type ServiceOption func(*Service)

func WithEvents(events resourceevents.Emitter) ServiceOption {
	return func(service *Service) {
		service.events = events
	}
}

type ExecuteInput struct {
	Method  string
	URL     string
	Headers []Header
	Body    proxybody.Body
}

type Header struct {
	Name  string `json:"name"`
	Value string `json:"value"`
}

type ExecuteResult struct {
	Status         int      `json:"status"`
	StatusText     string   `json:"status_text"`
	HTTPVersion    string   `json:"http_version"`
	FinalURL       string   `json:"final_url"`
	Headers        []Header `json:"headers"`
	ContentType    string   `json:"content_type,omitempty"`
	BodyBase64     string   `json:"body_base64"`
	DurationMicros int64    `json:"duration_micros"`
}

type hostnameOverridesContextKey struct{}
type bypassProxyContextKey struct{}
type requestTargetHostContextKey struct{}

func NewService(
	workspaceService *workspaces.Service,
	settingsRepository *SettingsRepository,
	options ...ServiceOption,
) *Service {
	transport := transportWithHostnameOverrides(http.DefaultTransport.(*http.Transport))
	service := &Service{
		workspaces: workspaceService,
		settings:   settingsRepository,
		client: &http.Client{
			Transport: transport,
			Timeout:   DefaultTimeout,
			CheckRedirect: func(request *http.Request, via []*http.Request) error {
				if len(via) >= MaxRedirects {
					return fmt.Errorf("stopped after %d redirects", MaxRedirects)
				}
				applyHostnameOriginOverride(request)
				return nil
			},
		},
	}
	for _, option := range options {
		option(service)
	}
	return service
}

func transportWithHostnameOverrides(base *http.Transport) *http.Transport {
	transport := base.Clone()
	defaultProxy := transport.Proxy
	baseDialContext := transport.DialContext
	if baseDialContext == nil {
		dialer := &net.Dialer{
			Timeout:   30 * time.Second,
			KeepAlive: 30 * time.Second,
		}
		baseDialContext = dialer.DialContext
	}
	transport.Proxy = func(request *http.Request) (*url.URL, error) {
		bypassHost, _ := request.Context().Value(bypassProxyContextKey{}).(string)
		if bypassHost != "" && bypassHost == normalizedHostname(request.URL.Hostname()) {
			return nil, nil
		}
		if defaultProxy == nil {
			return nil, nil
		}
		return defaultProxy(request)
	}
	transport.DialContext = func(ctx context.Context, network, address string) (net.Conn, error) {
		host, port, err := net.SplitHostPort(address)
		if err != nil {
			return baseDialContext(ctx, network, address)
		}
		originalHost, _ := ctx.Value(requestTargetHostContextKey{}).(string)
		if originalHost != "" {
			if _, overridden := hostnameOverride(ctx, originalHost); overridden {
				// An admin-configured hostname override is the documented way to
				// reach private-network destinations; trust the configured target.
				if target, rewritten := hostnameOverride(ctx, host); rewritten && net.ParseIP(target.Host) != nil {
					address = net.JoinHostPort(target.Host, port)
				}
				return baseDialContext(ctx, network, address)
			}
			// No override: enforce egress restrictions. Literal targets are
			// checked directly; hostnames are resolved and every address
			// validated, then the validated address is dialed so the destination
			// cannot change between the check and the connection (DNS rebinding).
			if dialAddress, reason := validatedDestination(ctx, host, port); reason != "" {
				return nil, errors.New(reason)
			} else if dialAddress != "" {
				address = dialAddress
			}
			return baseDialContext(ctx, network, address)
		}
		// Requests that did not originate from Execute (e.g. transport-level
		// tests) keep the legacy override-only behavior.
		if target, overridden := hostnameOverride(ctx, host); overridden && net.ParseIP(target.Host) != nil {
			address = net.JoinHostPort(target.Host, port)
		}
		return baseDialContext(ctx, network, address)
	}
	return transport
}

// validatedDestination returns the concrete "host:port" to dial plus an empty
// reason when the destination is allowed, or an empty address plus a non-empty
// reason when it must be blocked.
func validatedDestination(ctx context.Context, host, port string) (string, string) {
	if ip := net.ParseIP(host); ip != nil {
		if reason := blockedIPReason(ip); reason != "" {
			return "", reason
		}
		return net.JoinHostPort(ip.String(), port), ""
	}
	addresses, err := net.DefaultResolver.LookupIPAddr(ctx, host)
	if err != nil || len(addresses) == 0 {
		// Resolution errors surface from the dialer; nothing to block here.
		return "", ""
	}
	for _, resolved := range addresses {
		if reason := blockedIPReason(resolved.IP); reason != "" {
			return "", fmt.Sprintf("destination %q resolves to a blocked address (%s)", host, reason)
		}
	}
	// Dial a validated address directly so a second resolution cannot change it.
	return net.JoinHostPort(addresses[0].IP.String(), port), ""
}

// blockedIPReason returns a description when `ip` must not be reachable through
// the request proxy, or "" when it is allowed.
func blockedIPReason(ip net.IP) string {
	if ip.IsLoopback() {
		return "loopback addresses"
	}
	if ip.IsLinkLocalUnicast() || ip.IsLinkLocalMulticast() {
		return "link-local addresses"
	}
	if ip.IsPrivate() {
		return "private-network addresses"
	}
	if ip.IsUnspecified() || ip.IsMulticast() {
		return "unspecified or multicast addresses"
	}
	if ip4 := ip.To4(); ip4 != nil && ip4[0] == 100 && ip4[1] >= 64 && ip4[1] <= 127 {
		return "shared-address-space (CGNAT) addresses"
	}
	return ""
}

func (s *Service) Policy(ctx context.Context) (Policy, error) {
	settings, err := s.settings.Get(ctx)
	if err != nil {
		return Policy{}, err
	}
	return Policy{Mode: settings.Mode}, nil
}

func (s *Service) Settings(ctx context.Context) (Settings, error) {
	return s.settings.Get(ctx)
}

func (s *Service) UpdateSettings(ctx context.Context, settings Settings) (Settings, error) {
	updated, err := s.settings.Replace(ctx, settings)
	if err != nil {
		return Settings{}, err
	}
	// A pooled connection is keyed by the request origin, not by the overridden
	// dial target. Discard idle connections so an updated override takes effect
	// on the next request instead of reusing the previous destination.
	s.client.CloseIdleConnections()
	return updated, nil
}

func (s *Service) Execute(
	ctx context.Context,
	actor workspaces.Actor,
	workspaceID string,
	input ExecuteInput,
) (ExecuteResult, error) {
	if _, err := s.workspaces.Get(ctx, actor, workspaceID); err != nil {
		return ExecuteResult{}, err
	}
	settings, err := s.settings.Get(ctx)
	if err != nil {
		return ExecuteResult{}, err
	}
	if settings.Mode != ModeServer {
		return ExecuteResult{}, problem.New(
			problem.KindConflict,
			"server_execution_disabled",
			"request execution from this server is disabled by its administrator",
		)
	}

	overrides := settings.overrideMap()
	target, err := resolveRequestURL(input.URL, overrides)
	if err != nil {
		return ExecuteResult{}, err
	}
	// Reject literal loopback/link-local/private destinations up front (hostname
	// targets are enforced at dial time) unless an admin override covers them.
	if ip := net.ParseIP(target.Hostname()); ip != nil {
		if _, overridden := overrides[normalizedHostname(target.Hostname())]; !overridden {
			if reason := blockedIPReason(ip); reason != "" {
				return ExecuteResult{}, invalidField("url", reason)
			}
		}
	}

	method := strings.ToUpper(strings.TrimSpace(input.Method))
	if method == "" {
		return ExecuteResult{}, invalidField("method", "is required")
	}

	body, contentType, err := proxybody.Build(input.Body)
	if err != nil {
		return ExecuteResult{}, err
	}
	requestContext := context.WithValue(ctx, hostnameOverridesContextKey{}, overrides)
	requestContext = context.WithValue(
		requestContext,
		requestTargetHostContextKey{},
		normalizedHostname(target.Hostname()),
	)
	request, err := http.NewRequestWithContext(requestContext, method, target.String(), bytes.NewReader(body))
	if err != nil {
		return ExecuteResult{}, invalidField("method", "is not a valid HTTP method")
	}
	if err := applyHeaders(request, input.Headers); err != nil {
		return ExecuteResult{}, err
	}
	request.Header.Del("Content-Length")
	request.Header.Del("Transfer-Encoding")
	if contentType != "" && (input.Body.Mode == "multipart_form_data" || request.Header.Get("Content-Type") == "") {
		request.Header.Set("Content-Type", contentType)
	}
	applyHostnameOriginOverride(request)

	startedAt := time.Now()
	// Record the audit entry exactly once per execution. The status is
	// captured after the request is sent; transport failures keep it at 0,
	// matching the historical behavior of recording on every return path.
	statusToRecord := 0
	defer func() {
		s.recordExecution(requestContext, actor, workspaceID, method, target, statusToRecord, time.Since(startedAt))
	}()

	response, err := s.client.Do(request)
	if err != nil {
		return ExecuteResult{}, proxyTransportError(err)
	}
	defer response.Body.Close()
	statusToRecord = response.StatusCode

	if response.ContentLength > MaxResponseBodyBytes {
		return ExecuteResult{}, proxyResponseTooLarge()
	}
	responseBody, err := io.ReadAll(io.LimitReader(response.Body, MaxResponseBodyBytes+1))
	if err != nil {
		return ExecuteResult{}, proxyTransportError(err)
	}
	if len(responseBody) > MaxResponseBodyBytes {
		return ExecuteResult{}, proxyResponseTooLarge()
	}

	return ExecuteResult{
		Status:         response.StatusCode,
		StatusText:     http.StatusText(response.StatusCode),
		HTTPVersion:    response.Proto,
		FinalURL:       response.Request.URL.String(),
		Headers:        responseHeaders(response.Header),
		ContentType:    response.Header.Get("Content-Type"),
		BodyBase64:     base64.StdEncoding.EncodeToString(responseBody),
		DurationMicros: time.Since(startedAt).Microseconds(),
	}, nil
}

func resolveRequestURL(value string, overrides map[string]hostnameOverrideTarget) (*url.URL, error) {
	raw := strings.TrimSpace(value)
	target, err := url.Parse(raw)
	if err != nil {
		return nil, invalidField("url", "is not a valid URL")
	}
	if target.Host == "" && !strings.Contains(raw, "://") {
		target, err = url.Parse("//" + strings.TrimPrefix(raw, "//"))
		if err != nil {
			return nil, invalidField("url", "is not a valid URL")
		}
	}
	if target.Host == "" {
		return nil, invalidField("url", "must include a hostname")
	}
	if target.Scheme == "" {
		override, overridden := overrides[normalizedHostname(target.Hostname())]
		if !overridden || override.Scheme == "" {
			return nil, invalidField(
				"url",
				"must include http:// or https:// unless its hostname override supplies a scheme",
			)
		}
		target.Scheme = override.Scheme
	} else if target.Scheme != "http" && target.Scheme != "https" {
		return nil, invalidField("url", "must use HTTP or HTTPS")
	}
	return target, nil
}

func hostnameOverride(ctx context.Context, hostname string) (hostnameOverrideTarget, bool) {
	overrides, _ := ctx.Value(hostnameOverridesContextKey{}).(map[string]hostnameOverrideTarget)
	target, ok := overrides[normalizedHostname(hostname)]
	return target, ok
}

func applyHostnameOriginOverride(request *http.Request) {
	target, overridden := hostnameOverride(request.Context(), request.URL.Hostname())
	if !overridden {
		return
	}

	requestURL := *request.URL
	request.URL = &requestURL
	if target.Scheme != "" {
		request.URL.Scheme = target.Scheme
	}
	if net.ParseIP(target.Host) != nil {
		markProxyBypass(request, request.URL.Hostname())
		return
	}

	if port := request.URL.Port(); port != "" {
		request.URL.Host = net.JoinHostPort(target.Host, port)
	} else {
		request.URL.Host = target.Host
	}
	request.Host = request.URL.Host
	markProxyBypass(request, target.Host)
}

func markProxyBypass(request *http.Request, hostname string) {
	*request = *request.WithContext(context.WithValue(
		request.Context(),
		bypassProxyContextKey{},
		normalizedHostname(hostname),
	))
}

func normalizedHostname(hostname string) string {
	return strings.ToLower(strings.TrimSuffix(hostname, "."))
}

func applyHeaders(request *http.Request, headers []Header) error {
	for _, header := range headers {
		name := strings.TrimSpace(header.Name)
		if name == "" {
			continue
		}
		if textproto.CanonicalMIMEHeaderKey(name) == "" || strings.ContainsAny(header.Value, "\r\n") {
			return invalidField("headers", fmt.Sprintf("contains an invalid %q header", header.Name))
		}
		switch strings.ToLower(name) {
		case "connection", "proxy-connection", "keep-alive", "transfer-encoding", "upgrade", "te", "trailer":
			continue
		case "host":
			request.Host = header.Value
		case "content-length":
			continue
		case "x-forwarded-for", "x-forwarded-host", "x-forwarded-proto", "forwarded":
			// Clients must not spoof internal routing headers; the server sets
			// these itself when needed.
			continue
		default:
			request.Header.Add(name, header.Value)
		}
	}
	return nil
}

func responseHeaders(headers http.Header) []Header {
	result := make([]Header, 0, len(headers))
	for name, values := range headers {
		for _, value := range values {
			result = append(result, Header{Name: name, Value: value})
		}
	}
	return result
}

func proxyTransportError(err error) error {
	if errors.Is(err, context.DeadlineExceeded) {
		return problem.New(problem.KindGatewayTimeout, "proxy_timeout", "the proxied request timed out")
	}
	var networkError net.Error
	if errors.As(err, &networkError) && networkError.Timeout() {
		return problem.New(problem.KindGatewayTimeout, "proxy_timeout", "the proxied request timed out")
	}
	return problem.New(problem.KindBadGateway, "proxy_request_failed", fmt.Sprintf("the proxied request failed: %v", err))
}

// recordExecution writes an audit entry for a proxied request: actor, method,
// destination host, status, and duration. Payloads and headers are never
// recorded.
func (s *Service) recordExecution(
	ctx context.Context,
	actor workspaces.Actor,
	workspaceID string,
	method string,
	target *url.URL,
	status int,
	duration time.Duration,
) {
	if s.events == nil {
		return
	}
	resourceevents.Emit(s.events, resourceevents.Change{
		Resource:    resourceevents.ResourceRequestExecution,
		Action:      resourceevents.ActionExecuted,
		ResourceID:  uuid.NewString(),
		WorkspaceID: workspaceID,
		ActorUserID: actor.UserID,
		TargetName:  target.Host,
		Diffs: []resourceevents.Diff{
			{Field: "method", From: "", To: method},
			{Field: "host", From: "", To: target.Host},
			{Field: "status", From: "", To: strconv.Itoa(status)},
			{Field: "duration_micros", From: "", To: strconv.FormatInt(duration.Microseconds(), 10)},
		},
	})
}

func proxyResponseTooLarge() error {
	return problem.New(
		problem.KindPayloadTooLarge,
		"proxy_response_too_large",
		fmt.Sprintf("the proxied response exceeds the %d-byte limit", MaxResponseBodyBytes),
	)
}

func invalidField(field, message string) error {
	return problem.WithFields(
		"validation_failed",
		"request validation failed",
		map[string]string{field: message},
	)
}
