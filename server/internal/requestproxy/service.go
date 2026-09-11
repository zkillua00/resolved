package requestproxy

import (
	"bytes"
	"context"
	"crypto/tls"
	"encoding/base64"
	"errors"
	"fmt"
	"net"
	"net/http"
	"net/textproto"
	"net/url"
	"strconv"
	"strings"
	"time"

	gorillaWebsocket "github.com/gorilla/websocket"
	"resolved-server/internal/executionlimits"
	"resolved-server/internal/identity"
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
	proxies    *ProxyRepository
	client     *http.Client
	events     resourceevents.Emitter
	limits     *executionlimits.Provider
}

type ServiceOption func(*Service)

func WithEvents(events resourceevents.Emitter) ServiceOption {
	return func(service *Service) {
		service.events = events
	}
}

type ExecuteInput struct {
	CollectionID string
	UseCookieJar bool
	// RequestID optionally names the saved request being executed so proxy
	// resolution can apply request- and collection-scoped proxies.
	RequestID string
	Method    string
	URL       string
	Headers   []Header
	Body      proxybody.Body
}

type WebSocketOpenInput struct {
	RequestID    string   `json:"request_id"`
	CollectionID string   `json:"collection_id,omitempty"`
	URL          string   `json:"url"`
	Headers      []Header `json:"headers"`
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
type requestTargetURLContextKey struct{}
type allowlistedRequestsContextKey struct{}
type allowlistedAddressesContextKey struct{}

type blockedDestinationError struct {
	Request string
	Address string
	Reason  string
}

func (e *blockedDestinationError) Error() string {
	return fmt.Sprintf("destination %q is blocked (%s)", e.Address, e.Reason)
}

func NewService(
	workspaceService *workspaces.Service,
	settingsRepository *SettingsRepository,
	proxyRepository *ProxyRepository,
	options ...ServiceOption,
) *Service {
	transport := transportWithHostnameOverrides(http.DefaultTransport.(*http.Transport))
	service := &Service{
		workspaces: workspaceService,
		settings:   settingsRepository,
		proxies:    proxyRepository,
		limits:     executionlimits.NewProvider(settingsRepository.db),
		client: &http.Client{
			Transport: transport,
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
			if dialAddress, blocked := validatedDestination(ctx, host, port); blocked != nil {
				return nil, blocked
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

// validatedDestination returns the concrete "host:port" to dial when the
// destination is allowed, or structured details for a blocked destination.
func validatedDestination(ctx context.Context, host, port string) (string, *blockedDestinationError) {
	requestURL, _ := ctx.Value(requestTargetURLContextKey{}).(string)
	requests, _ := ctx.Value(allowlistedRequestsContextKey{}).(map[string]struct{})
	allowlistedAddresses, _ := ctx.Value(allowlistedAddressesContextKey{}).(map[string]struct{})
	_, requestAllowed := requests[requestURL]
	_, hostAllowed := allowlistedAddresses[normalizedHostname(host)]
	blocked := func(address, reason string) (string, *blockedDestinationError) {
		return "", &blockedDestinationError{Request: requestURL, Address: address, Reason: reason}
	}
	if ip := net.ParseIP(host); ip != nil {
		if reason := blockedIPReason(ip); reason != "" && !requestAllowed && !hostAllowed {
			return blocked(ip.String(), reason)
		}
		return net.JoinHostPort(ip.String(), port), nil
	}
	resolvedAddresses, err := net.DefaultResolver.LookupIPAddr(ctx, host)
	if err != nil || len(resolvedAddresses) == 0 {
		// Resolution errors surface from the dialer; nothing to block here.
		return "", nil
	}
	for _, resolved := range resolvedAddresses {
		_, resolvedAllowed := allowlistedAddresses[resolved.IP.String()]
		if reason := blockedIPReason(resolved.IP); reason != "" && !requestAllowed && !hostAllowed && !resolvedAllowed {
			return blocked(resolved.IP.String(), fmt.Sprintf("%q resolves to a blocked address (%s)", host, reason))
		}
	}
	// Dial a validated address directly so a second resolution cannot change it.
	return net.JoinHostPort(resolvedAddresses[0].IP.String(), port), nil
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
	cached, ok := ctx.Value(executionSnapshotKey{}).(executionSnapshot)
	snapshot := cached.snapshot
	if !ok {
		snapshot, err = s.limits.Resolve(ctx, executionlimits.Scope{})
		if err != nil {
			return Policy{}, err
		}
	}
	return Policy{Mode: settings.Mode, CookieJar: true, Limits: snapshot.Effective}, nil
}

func (s *Service) Settings(ctx context.Context) (Settings, error) {
	return s.settings.Get(ctx)
}

func (s *Service) UpdateSettings(
	ctx context.Context,
	actorUserID string,
	settings Settings,
) (Settings, error) {
	before, err := s.settings.Get(ctx)
	if err != nil {
		return Settings{}, err
	}
	updated, err := s.settings.Replace(ctx, settings)
	if err != nil {
		return Settings{}, err
	}
	// A pooled connection is keyed by the request origin, not by the overridden
	// dial target. Discard idle connections so an updated override takes effect
	// on the next request instead of reusing the previous destination.
	s.client.CloseIdleConnections()
	resourceevents.Emit(s.events, resourceevents.Change{
		Resource:    resourceevents.ResourceServerSettings,
		Action:      resourceevents.ActionUpdated,
		ResourceID:  SettingsRecordID,
		ActorUserID: actorUserID,
		TargetName:  "request execution settings",
		Audience: resourceevents.Audience{
			PermissionKeys: []string{identity.PermissionServerSettingsRead},
		},
		Diffs: []resourceevents.Diff{
			{Field: "mode", From: before.Mode, To: updated.Mode},
		},
	})
	return updated, nil
}

func (s *Service) AddAllowlistEntry(ctx context.Context, actorUserID string, entry AllowlistEntry) (AllowlistEntry, error) {
	normalized, err := normalizeAllowlistEntry(entry)
	if err != nil {
		return AllowlistEntry{}, err
	}
	if _, err := s.settings.AddAllowlistEntry(ctx, normalized); err != nil {
		return AllowlistEntry{}, err
	}
	s.client.CloseIdleConnections()
	resourceevents.Emit(s.events, resourceevents.Change{
		Resource: resourceevents.ResourceServerSettings, Action: resourceevents.ActionUpdated,
		ResourceID: SettingsRecordID, ActorUserID: actorUserID,
		TargetName: "request destination allowlist",
		Audience:   resourceevents.Audience{PermissionKeys: []string{identity.PermissionServerSettingsRead}},
		Diffs:      []resourceevents.Diff{{Field: "allowlist_" + normalized.Kind, From: "", To: "added"}},
	})
	return normalized, nil
}

func (s *Service) Execute(
	ctx context.Context,
	actor workspaces.Actor,
	workspaceID string,
	input ExecuteInput,
) (ExecuteResult, error) {
	workspace, err := s.workspaces.Get(ctx, actor, workspaceID)
	if err != nil {
		return ExecuteResult{}, err
	}
	snapshot, err := s.resolveLimits(ctx, actor, workspaceID, input.CollectionID)
	if err != nil {
		return ExecuteResult{}, err
	}
	if err := validateDescriptor(snapshot, input.URL, input.Headers); err != nil {
		return ExecuteResult{}, err
	}
	ctx, cancel := executionContext(ctx, snapshot.Effective["http.timeout_ms"])
	defer cancel()
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

	overrides, err := s.effectiveOverrides(ctx, actor, workspace, input.RequestID)
	if err != nil {
		return ExecuteResult{}, err
	}
	allowlistedRequests, allowlistedAddresses := settings.allowlistMaps()
	target, err := resolveRequestURL(input.URL, overrides)
	if err != nil {
		return ExecuteResult{}, err
	}
	// Reject literal loopback/link-local/private destinations up front (hostname
	// targets are enforced at dial time) unless an admin override covers them.
	if ip := net.ParseIP(target.Hostname()); ip != nil {
		if _, overridden := overrides[normalizedHostname(target.Hostname())]; !overridden {
			requestTarget := *target
			requestTarget.Fragment = ""
			_, requestAllowed := allowlistedRequests[requestTarget.String()]
			_, addressAllowed := allowlistedAddresses[normalizedHostname(target.Hostname())]
			if reason := blockedIPReason(ip); reason != "" && !requestAllowed && !addressAllowed {
				return ExecuteResult{}, problem.WithFields(
					"proxy_destination_blocked",
					"the proxied request was blocked because its destination is not allowed",
					map[string]string{
						"request": requestTarget.String(),
						"address": normalizedHostname(target.Hostname()),
						"reason":  reason,
					},
				)
			}
		}
	}

	method := strings.ToUpper(strings.TrimSpace(input.Method))
	if method == "" {
		return ExecuteResult{}, invalidField("method", "is required")
	}

	bodyBound := snapshot.Effective["http.request_bytes"]
	body, contentType, err := proxybody.BuildWithLimit(input.Body, proxybody.Limit{Unlimited: bodyBound.Unlimited, Value: bodyBound.Value})
	if err != nil {
		return ExecuteResult{}, err
	}
	requestContext := context.WithValue(ctx, hostnameOverridesContextKey{}, overrides)
	requestContext = context.WithValue(requestContext, allowlistedRequestsContextKey{}, allowlistedRequests)
	requestContext = context.WithValue(requestContext, allowlistedAddressesContextKey{}, allowlistedAddresses)
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
	setRequestTargetContext(request)
	applyHostnameOriginOverride(request)

	startedAt := time.Now()
	// Record the audit entry exactly once per execution. The status is
	// captured after the request is sent; transport failures keep it at 0,
	// matching the historical behavior of recording on every return path.
	statusToRecord := 0
	defer func() {
		s.recordExecution(requestContext, actor, workspaceID, method, target, statusToRecord, time.Since(startedAt))
	}()

	client := *s.client
	client.Timeout = 0
	if base, ok := client.Transport.(*http.Transport); ok {
		transport := executionTransport(base, snapshot)
		defer transport.CloseIdleConnections()
		client.Transport = transport
	}
	client.CheckRedirect = func(request *http.Request, via []*http.Request) error {
		bound := snapshot.Effective["http.redirects"]
		if !bound.Unlimited && int64(len(via)) > bound.Value {
			return fmt.Errorf("redirect limit exceeded")
		}
		if err := validateDescriptor(snapshot, request.URL.String(), nil); err != nil {
			return err
		}
		setRequestTargetContext(request)
		applyHostnameOriginOverride(request)
		return nil
	}
	var cookies *executionCookieJar
	if input.UseCookieJar {
		snapshot, err := s.workspaces.GetCookieJar(ctx, actor, workspaceID)
		if err != nil {
			return ExecuteResult{}, err
		}
		if snapshot.Enabled {
			_, explicit := request.Header["Cookie"]
			cookies = newExecutionCookieJar(snapshot, explicit)
			client.Jar = cookies
		}
	}
	response, err := client.Do(request)
	if response != nil {
		statusToRecord = response.StatusCode
	}
	// Persist redirect cookies even when the final transport or body fails.
	if cookies != nil && len(cookies.updates) > 0 {
		saveContext, cancel := context.WithTimeout(context.WithoutCancel(ctx), 5*time.Second)
		saveErr := s.workspaces.ApplyCookieUpdates(saveContext, actor, workspaceID, cookies.updates)
		cancel()
		if saveErr != nil {
			if response != nil {
				response.Body.Close()
			}
			return ExecuteResult{}, problem.Wrap(saveErr, "save response cookies; request may have been sent")
		}
	}
	if err != nil {
		return ExecuteResult{}, proxyTransportError(err)
	}
	defer response.Body.Close()
	statusToRecord = response.StatusCode

	if exceeds(snapshot.Effective["http.response_bytes"], response.ContentLength) {
		return ExecuteResult{}, proxyResponseTooLarge()
	}
	responseBody, tooLarge, err := readLimited(response.Body, snapshot.Effective["http.response_bytes"])
	if err != nil {
		return ExecuteResult{}, proxyTransportError(err)
	}
	if tooLarge {
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

// OpenWebSocket applies the same workspace, execution-policy, destination,
// header, proxy, and hostname-override boundary as Execute before dialing a
// user-supplied WebSocket target.
func (s *Service) OpenWebSocket(
	ctx context.Context,
	actor workspaces.Actor,
	workspaceID string,
	input WebSocketOpenInput,
) (*gorillaWebsocket.Conn, *url.URL, error) {
	workspace, err := s.workspaces.Get(ctx, actor, workspaceID)
	if err != nil {
		return nil, nil, err
	}
	snapshot, err := s.resolveLimits(ctx, actor, workspaceID, input.CollectionID)
	if err != nil {
		return nil, nil, err
	}
	if err := validateDescriptor(snapshot, input.URL, input.Headers); err != nil {
		return nil, nil, err
	}
	ctx, cancel := executionContext(ctx, snapshot.Effective["websocket.handshake_timeout_ms"])
	defer cancel()
	settings, err := s.settings.Get(ctx)
	if err != nil {
		return nil, nil, err
	}
	if settings.Mode != ModeServer {
		return nil, nil, problem.New(
			problem.KindConflict,
			"server_execution_disabled",
			"request execution from this server is disabled by its administrator",
		)
	}

	target, err := url.Parse(strings.TrimSpace(input.URL))
	if err != nil || target.Host == "" {
		return nil, nil, invalidField("url", "is not a valid WebSocket URL")
	}
	if target.Scheme != "ws" && target.Scheme != "wss" {
		return nil, nil, invalidField("url", "must use ws:// or wss://")
	}
	originalTarget := *target
	originalTarget.Fragment = ""

	overrides, err := s.effectiveOverrides(ctx, actor, workspace, input.RequestID)
	if err != nil {
		return nil, nil, err
	}
	allowlistedRequests, allowlistedAddresses := settings.allowlistMaps()
	if ip := net.ParseIP(target.Hostname()); ip != nil {
		if _, overridden := overrides[normalizedHostname(target.Hostname())]; !overridden {
			_, requestAllowed := allowlistedRequests[originalTarget.String()]
			_, addressAllowed := allowlistedAddresses[normalizedHostname(target.Hostname())]
			if reason := blockedIPReason(ip); reason != "" && !requestAllowed && !addressAllowed {
				return nil, nil, problem.WithFields(
					"proxy_destination_blocked",
					"the proxied request was blocked because its destination is not allowed",
					map[string]string{
						"request": originalTarget.String(),
						"address": normalizedHostname(target.Hostname()),
						"reason":  reason,
					},
				)
			}
		}
	}

	requestContext := context.WithValue(ctx, hostnameOverridesContextKey{}, overrides)
	requestContext = context.WithValue(requestContext, allowlistedRequestsContextKey{}, allowlistedRequests)
	requestContext = context.WithValue(requestContext, allowlistedAddressesContextKey{}, allowlistedAddresses)
	request, err := http.NewRequestWithContext(requestContext, http.MethodGet, target.String(), nil)
	if err != nil {
		return nil, nil, invalidField("url", "is not a valid WebSocket URL")
	}
	if err := applyHeaders(request, input.Headers); err != nil {
		return nil, nil, err
	}
	// Gorilla owns the WebSocket handshake headers. Target subprotocols and
	// ordinary request headers remain caller-controlled.
	request.Header.Del("Sec-WebSocket-Key")
	request.Header.Del("Sec-WebSocket-Version")
	request.Header.Del("Sec-WebSocket-Extensions")
	setRequestTargetContext(request)
	applyWebSocketHostnameOriginOverride(request)
	if request.Host != "" {
		request.Header["Host"] = []string{request.Host}
	}

	transport, ok := s.client.Transport.(*http.Transport)
	if !ok {
		return nil, nil, problem.New(problem.KindInternal, "proxy_unavailable", "the request proxy transport is unavailable")
	}
	dialer := *gorillaWebsocket.DefaultDialer
	transport = executionTransport(transport, snapshot)
	defer transport.CloseIdleConnections()
	dialer.HandshakeTimeout = limitDuration(snapshot.Effective["websocket.handshake_timeout_ms"])
	dialer.Proxy = webSocketProxySelector(transport.Proxy)
	dialer.NetDialContext = transport.DialContext
	dialer.TLSClientConfig = webSocketTLSClientConfig(transport.TLSClientConfig)
	connection, _, err := dialer.DialContext(request.Context(), request.URL.String(), request.Header)
	if err != nil {
		return nil, nil, proxyTransportError(err)
	}
	return connection, &originalTarget, nil
}

func webSocketTLSClientConfig(base *tls.Config) *tls.Config {
	if base == nil {
		return nil
	}
	config := base.Clone()
	// A WebSocket opening handshake is HTTP/1.1. The HTTP transport may have
	// added h2 to its shared TLS configuration, but Gorilla cannot speak HTTP/2
	// after ALPN selects it.
	config.NextProtos = []string{"http/1.1"}
	return config
}

func webSocketProxySelector(
	selector func(*http.Request) (*url.URL, error),
) func(*http.Request) (*url.URL, error) {
	if selector == nil {
		return nil
	}
	return func(request *http.Request) (*url.URL, error) {
		proxyRequest := request.Clone(request.Context())
		proxyURL := *request.URL
		if proxyURL.Scheme == "wss" {
			proxyURL.Scheme = "https"
		} else {
			proxyURL.Scheme = "http"
		}
		proxyRequest.URL = &proxyURL
		return selector(proxyRequest)
	}
}

func applyWebSocketHostnameOriginOverride(request *http.Request) {
	target, overridden := hostnameOverride(request.Context(), request.URL.Hostname())
	if !overridden {
		return
	}
	requestURL := *request.URL
	request.URL = &requestURL
	if target.Scheme != "" {
		if target.Scheme == "https" {
			request.URL.Scheme = "wss"
		} else {
			request.URL.Scheme = "ws"
		}
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

// effectiveOverrides resolves the hostname overrides for one execution by
// walking the proxy scope chain from the most specific assigned scope to the
// server-wide scope. The workspace aggregate is already scoped to the actor,
// so a request the actor cannot see cannot pull in its proxies either.
func (s *Service) effectiveOverrides(
	ctx context.Context,
	actor workspaces.Actor,
	workspace workspaces.Workspace,
	requestID string,
) (map[string]hostnameOverrideTarget, error) {
	chain, err := proxyScopeChain(workspace, requestID)
	if err != nil {
		return nil, err
	}
	if s.proxies == nil {
		return map[string]hostnameOverrideTarget{}, nil
	}
	return s.proxies.EffectiveOverrides(ctx, chain, actor.UserID, actor.RoleIDs)
}

// proxyScopeChain orders the scopes that may carry a proxy for one execution:
// the saved request, its collection ancestors from innermost to outermost,
// the workspace, and finally the server-wide scope.
func proxyScopeChain(workspace workspaces.Workspace, requestID string) ([]ProxyScopeRef, error) {
	chain := make([]ProxyScopeRef, 0, 8)
	if requestID != "" {
		path, found := findRequestCollectionPath(workspace.Collections, requestID)
		if !found {
			return nil, invalidField("request_id", "does not reference an accessible saved request in this workspace")
		}
		chain = append(chain, ProxyScopeRef{Kind: ProxyScopeRequest, ID: requestID})
		for index := len(path) - 1; index >= 0; index-- {
			chain = append(chain, ProxyScopeRef{Kind: ProxyScopeCollection, ID: path[index]})
		}
	}
	chain = append(chain,
		ProxyScopeRef{Kind: ProxyScopeWorkspace, ID: workspace.ID},
		ProxyScopeRef{Kind: ProxyScopeServer, ID: ""},
	)
	return chain, nil
}

// findRequestCollectionPath returns the collection IDs from the workspace
// root down to the collection holding the request.
func findRequestCollectionPath(collections []workspaces.Collection, requestID string) ([]string, bool) {
	for _, collection := range collections {
		for _, request := range collection.Requests {
			if request.ID == requestID {
				return []string{collection.ID}, true
			}
		}
		if path, found := findRequestCollectionPath(collection.SubCollections, requestID); found {
			return append([]string{collection.ID}, path...), true
		}
	}
	return nil, false
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

func setRequestTargetContext(request *http.Request) {
	targetURL := *request.URL
	targetURL.Fragment = ""
	ctx := context.WithValue(request.Context(), requestTargetHostContextKey{}, normalizedHostname(request.URL.Hostname()))
	ctx = context.WithValue(ctx, requestTargetURLContextKey{}, targetURL.String())
	*request = *request.WithContext(ctx)
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
	var blocked *blockedDestinationError
	if errors.As(err, &blocked) {
		return problem.WithFields(
			"proxy_destination_blocked",
			"the proxied request was blocked because its destination is not allowed",
			map[string]string{
				"request": blocked.Request, "address": blocked.Address, "reason": blocked.Reason,
			},
		)
	}
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
		Audience: resourceevents.Audience{
			PermissionKeys: []string{identity.PermissionAuditRead},
		},
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
		"the proxied response exceeds its execution limit",
	)
}

func invalidField(field, message string) error {
	return problem.WithFields(
		"validation_failed",
		"request validation failed",
		map[string]string{field: message},
	)
}
