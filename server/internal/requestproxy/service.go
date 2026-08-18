package requestproxy

import (
	"bytes"
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"mime/multipart"
	"net"
	"net/http"
	"net/textproto"
	"net/url"
	"path/filepath"
	"strings"
	"time"

	"resolved-server/internal/problem"
	"resolved-server/internal/workspaces"
)

const (
	MaxRequestBodyBytes  = 64 * 1024 * 1024
	MaxResponseBodyBytes = 64 * 1024 * 1024
	DefaultTimeout       = 60 * time.Second
	MaxRedirects         = 10
)

type Service struct {
	workspaces *workspaces.Service
	settings   *SettingsRepository
	client     *http.Client
}

type ExecuteInput struct {
	Method  string
	URL     string
	Headers []Header
	Body    Body
}

type Header struct {
	Name  string `json:"name"`
	Value string `json:"value"`
}

type Body struct {
	Mode           string      `json:"mode"`
	RawContentType string      `json:"raw_content_type,omitempty"`
	DataBase64     string      `json:"data_base64,omitempty"`
	Fields         []BodyField `json:"fields,omitempty"`
}

type BodyField struct {
	Name          string `json:"name"`
	Kind          string `json:"kind"`
	Value         string `json:"value,omitempty"`
	Filename      string `json:"filename,omitempty"`
	ContentBase64 string `json:"content_base64,omitempty"`
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

func NewService(workspaceService *workspaces.Service, settingsRepository *SettingsRepository) *Service {
	transport := transportWithHostnameOverrides(http.DefaultTransport.(*http.Transport))
	return &Service{
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
		if err == nil {
			if target, overridden := hostnameOverride(ctx, host); overridden && net.ParseIP(target.Host) != nil {
				address = net.JoinHostPort(target.Host, port)
			}
		}
		return baseDialContext(ctx, network, address)
	}
	return transport
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

	method := strings.ToUpper(strings.TrimSpace(input.Method))
	if method == "" {
		return ExecuteResult{}, invalidField("method", "is required")
	}

	body, contentType, err := buildBody(input.Body)
	if err != nil {
		return ExecuteResult{}, err
	}
	requestContext := context.WithValue(ctx, hostnameOverridesContextKey{}, overrides)
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
	response, err := s.client.Do(request)
	if err != nil {
		return ExecuteResult{}, proxyTransportError(err)
	}
	defer response.Body.Close()

	if response.ContentLength > MaxResponseBodyBytes {
		return ExecuteResult{}, problem.New(
			problem.KindPayloadTooLarge,
			"proxy_response_too_large",
			fmt.Sprintf("the proxied response exceeds the %d-byte limit", MaxResponseBodyBytes),
		)
	}
	responseBody, err := io.ReadAll(io.LimitReader(response.Body, MaxResponseBodyBytes+1))
	if err != nil {
		return ExecuteResult{}, proxyTransportError(err)
	}
	if len(responseBody) > MaxResponseBodyBytes {
		return ExecuteResult{}, problem.New(
			problem.KindPayloadTooLarge,
			"proxy_response_too_large",
			fmt.Sprintf("the proxied response exceeds the %d-byte limit", MaxResponseBodyBytes),
		)
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

func buildBody(input Body) ([]byte, string, error) {
	switch input.Mode {
	case "none":
		return nil, "", nil
	case "raw":
		body, err := decodeBody(input.DataBase64, MaxRequestBodyBytes)
		if len(body) == 0 {
			return body, "", err
		}
		return body, input.RawContentType, err
	case "form_url_encoded":
		var encoded strings.Builder
		for _, field := range input.Fields {
			if field.Kind != "text" {
				return nil, "", invalidField("body.fields", "URL-encoded fields must contain text")
			}
			if encoded.Len() > 0 {
				encoded.WriteByte('&')
			}
			encoded.WriteString(url.QueryEscape(field.Name))
			encoded.WriteByte('=')
			encoded.WriteString(url.QueryEscape(field.Value))
			if encoded.Len() > MaxRequestBodyBytes {
				return nil, "", requestBodyTooLarge()
			}
		}
		return []byte(encoded.String()), "application/x-www-form-urlencoded", nil
	case "multipart_form_data":
		var encoded bytes.Buffer
		writer := multipart.NewWriter(&encoded)
		for _, field := range input.Fields {
			switch field.Kind {
			case "text":
				if err := writer.WriteField(field.Name, field.Value); err != nil {
					return nil, "", problem.Wrap(err, "encode multipart text field")
				}
			case "file":
				content, err := decodeBody(field.ContentBase64, MaxRequestBodyBytes)
				if err != nil {
					return nil, "", err
				}
				part, err := createFilePart(writer, field.Name, field.Filename)
				if err != nil {
					return nil, "", problem.Wrap(err, "encode multipart file field")
				}
				if _, err := part.Write(content); err != nil {
					return nil, "", problem.Wrap(err, "encode multipart file content")
				}
			default:
				return nil, "", invalidField("body.fields", "multipart fields must contain text or file data")
			}
			if encoded.Len() > MaxRequestBodyBytes {
				return nil, "", requestBodyTooLarge()
			}
		}
		if err := writer.Close(); err != nil {
			return nil, "", problem.Wrap(err, "finish multipart body")
		}
		if encoded.Len() > MaxRequestBodyBytes {
			return nil, "", requestBodyTooLarge()
		}
		return encoded.Bytes(), writer.FormDataContentType(), nil
	default:
		return nil, "", invalidField("body.mode", "is invalid")
	}
}

func createFilePart(writer *multipart.Writer, fieldName, filename string) (io.Writer, error) {
	header := make(textproto.MIMEHeader)
	header.Set(
		"Content-Disposition",
		fmt.Sprintf(`form-data; name=%q; filename=%q`, escapeQuotes(fieldName), escapeQuotes(filename)),
	)
	contentType := "application/octet-stream"
	if detected := mimeTypeForFilename(filename); detected != "" {
		contentType = detected
	}
	header.Set("Content-Type", contentType)
	return writer.CreatePart(header)
}

func mimeTypeForFilename(filename string) string {
	extension := strings.ToLower(filepath.Ext(filename))
	switch extension {
	case ".json":
		return "application/json"
	case ".xml":
		return "application/xml"
	case ".html", ".htm":
		return "text/html"
	case ".txt", ".md", ".csv":
		return "text/plain"
	case ".png":
		return "image/png"
	case ".jpg", ".jpeg":
		return "image/jpeg"
	case ".gif":
		return "image/gif"
	case ".pdf":
		return "application/pdf"
	default:
		return ""
	}
}

func escapeQuotes(value string) string {
	return strings.NewReplacer("\\", "\\\\", `"`, `\"`, "\r", "", "\n", "").Replace(value)
}

func decodeBody(encoded string, limit int) ([]byte, error) {
	if len(encoded) > base64.StdEncoding.EncodedLen(limit) {
		return nil, requestBodyTooLarge()
	}
	decoded, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		return nil, invalidField("body", "contains invalid base64 data")
	}
	if len(decoded) > limit {
		return nil, requestBodyTooLarge()
	}
	return decoded, nil
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

func requestBodyTooLarge() error {
	return problem.New(
		problem.KindPayloadTooLarge,
		"proxy_request_too_large",
		fmt.Sprintf("the proxied request body exceeds the %d-byte limit", MaxRequestBodyBytes),
	)
}

func invalidField(field, message string) error {
	return problem.WithFields(
		"validation_failed",
		"request validation failed",
		map[string]string{field: message},
	)
}
