package sharedhistory

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"strings"
	"time"

	"resolved-server/internal/problem"
	"resolved-server/internal/resourceevents"
	"resolved-server/internal/validation"
	"resolved-server/internal/workspaces"

	"github.com/google/uuid"
	"gorm.io/gorm"
)

type Service struct {
	repository *Repository
	workspaces *workspaces.Service
	events     resourceevents.Emitter
}

type ServiceOption func(*Service)

func WithEvents(events resourceevents.Emitter) ServiceOption {
	return func(service *Service) {
		service.events = events
	}
}

func NewService(
	repository *Repository,
	workspaceService *workspaces.Service,
	options ...ServiceOption,
) *Service {
	service := &Service{repository: repository, workspaces: workspaceService}
	for _, option := range options {
		option(service)
	}
	return service
}

// ListProfiles returns the user directory. Privileged callers (users.read or
// history.read_others) see the full directory with email and active status;
// everyone else sees only users they share a workspace with, with no email or
// active flag, to avoid exposing the whole account directory (and enabling
// email enumeration) to any authenticated account.
func (s *Service) ListProfiles(ctx context.Context, actor workspaces.Actor, privileged bool) ([]ProfileView, error) {
	users, err := s.repository.ListProfiles(ctx)
	if err != nil {
		return nil, problem.Wrap(err, "list profiles")
	}
	if privileged {
		profiles := make([]ProfileView, 0, len(users))
		for _, user := range users {
			profiles = append(profiles, ProfileView{
				ID: user.ID, Email: user.Email, DisplayName: user.DisplayName, Active: user.Active,
			})
		}
		return profiles, nil
	}
	visible := map[string]struct{}{actor.UserID: {}}
	accessible, err := s.workspaces.List(ctx, actor)
	if err != nil {
		return nil, problem.Wrap(err, "list accessible workspaces")
	}
	for _, workspace := range accessible {
		for _, memberID := range workspace.UserIDs {
			visible[memberID] = struct{}{}
		}
	}
	profiles := make([]ProfileView, 0, len(users))
	for _, user := range users {
		if _, ok := visible[user.ID]; !ok {
			continue
		}
		profiles = append(profiles, ProfileView{ID: user.ID, DisplayName: user.DisplayName})
	}
	return profiles, nil
}

func (s *Service) List(
	ctx context.Context,
	actor workspaces.Actor,
	canReadOthers bool,
	workspaceID, userID string,
) ([]EntryView, error) {
	if err := validation.ID("workspace_id", workspaceID); err != nil {
		return nil, err
	}
	if err := validation.ID("user_id", userID); err != nil {
		return nil, err
	}
	if _, err := s.workspaces.Get(ctx, actor, workspaceID); err != nil {
		return nil, err
	}
	if userID != actor.UserID && !canReadOthers {
		return nil, problem.New(
			problem.KindForbidden,
			"history_access_denied",
			"the account cannot view another user's history",
		)
	}
	if _, err := s.repository.GetProfile(ctx, userID); err != nil {
		if errors.Is(err, gorm.ErrRecordNotFound) {
			return nil, problem.New(problem.KindNotFound, "profile_not_found", "profile was not found")
		}
		return nil, problem.Wrap(err, "load history profile")
	}
	entries, err := s.repository.ListEntries(ctx, workspaceID, userID)
	if err != nil {
		return nil, problem.Wrap(err, "list shared history")
	}
	views := make([]EntryView, 0, len(entries))
	for _, entry := range entries {
		view, err := viewEntry(entry)
		if err != nil {
			return nil, problem.Wrap(err, "decode shared history")
		}
		views = append(views, view)
	}
	return views, nil
}

func (s *Service) Create(
	ctx context.Context,
	actor workspaces.Actor,
	workspaceID string,
	input CreateInput,
) (EntryView, error) {
	if err := validation.ID("workspace_id", workspaceID); err != nil {
		return EntryView{}, err
	}
	if _, err := s.workspaces.Get(ctx, actor, workspaceID); err != nil {
		return EntryView{}, err
	}
	viewerIDs, err := s.repository.ListRealtimeViewerUserIDs(ctx, workspaceID, actor.UserID)
	if err != nil {
		return EntryView{}, problem.Wrap(err, "resolve shared history viewers")
	}
	normalized, responseBody, err := normalizeInput(input)
	if err != nil {
		return EntryView{}, err
	}
	requestHeadersJSON, _ := json.Marshal(normalized.Request.Headers)
	requestFieldsJSON, _ := json.Marshal(normalized.Request.BodyFields)
	responseHeadersJSON := []byte("[]")
	var responseStatus *int
	var responseDuration *int64
	var responseStatusText, responseHTTPVersion, responseFinalURL, responseContentType string
	var responseBodyTruncated bool
	if normalized.Response != nil {
		responseHeadersJSON, _ = json.Marshal(normalized.Response.Headers)
		status := normalized.Response.Status
		responseStatus = &status
		duration := normalized.Response.DurationMicros
		responseDuration = &duration
		responseStatusText = normalized.Response.StatusText
		responseHTTPVersion = normalized.Response.HTTPVersion
		responseFinalURL = normalized.Response.FinalURL
		responseContentType = normalized.Response.ContentType
		responseBodyTruncated = normalized.Response.BodyTruncated
	}
	entry, err := s.repository.UpsertEntry(ctx, Entry{
		ID: uuid.NewString(), WorkspaceID: workspaceID, UserID: actor.UserID,
		ClientEntryID: normalized.ClientEntryID, Method: normalized.Request.Method,
		URL: normalized.Request.URL, RequestHeadersJSON: requestHeadersJSON,
		RequestBody: []byte(normalized.Request.Body), RequestBodyMode: normalized.Request.BodyMode,
		RequestBodyLanguage:   normalized.Request.BodyLanguage,
		RequestBodyFieldsJSON: requestFieldsJSON,
		RequestBodyTruncated:  normalized.Request.BodyTruncated,
		ResponseStatus:        responseStatus, ResponseStatusText: responseStatusText,
		ResponseHTTPVersion: responseHTTPVersion, ResponseFinalURL: responseFinalURL,
		ResponseHeadersJSON: responseHeadersJSON, ResponseBody: responseBody,
		ResponseBodyTruncated: responseBodyTruncated, ResponseContentType: responseContentType,
		ResponseDurationMicros: responseDuration, Error: normalized.Error,
		CreatedAt: normalized.CreatedAt.UTC(), UpdatedAt: time.Now().UTC(),
	})
	if err != nil {
		return EntryView{}, problem.Wrap(err, "save shared history")
	}
	view, err := viewEntry(entry)
	if err != nil {
		return EntryView{}, problem.Wrap(err, "decode saved shared history")
	}
	s.publishChange(resourceevents.ActionUpdated, workspaceID, actor.UserID, viewerIDs)
	return view, nil
}

func (s *Service) DeleteOwn(ctx context.Context, actor workspaces.Actor, workspaceID string) error {
	if err := validation.ID("workspace_id", workspaceID); err != nil {
		return err
	}
	if _, err := s.workspaces.Get(ctx, actor, workspaceID); err != nil {
		return err
	}
	viewerIDs, err := s.repository.ListRealtimeViewerUserIDs(ctx, workspaceID, actor.UserID)
	if err != nil {
		return problem.Wrap(err, "resolve shared history viewers")
	}
	if err := s.repository.DeleteEntries(ctx, workspaceID, actor.UserID); err != nil {
		return problem.Wrap(err, "delete shared history")
	}
	s.publishChange(resourceevents.ActionDeleted, workspaceID, actor.UserID, viewerIDs)
	return nil
}

func (s *Service) publishChange(
	action resourceevents.Action,
	workspaceID, historyOwnerID string,
	viewerIDs []string,
) {
	resourceevents.Emit(s.events, resourceevents.Change{
		Resource:    resourceevents.ResourceSharedHistory,
		Action:      action,
		ResourceID:  historyOwnerID,
		WorkspaceID: workspaceID,
		Audience: resourceevents.Audience{
			UserIDs: viewerIDs,
		},
	})
}

func normalizeInput(input CreateInput) (CreateInput, []byte, error) {
	input.ClientEntryID = strings.TrimSpace(input.ClientEntryID)
	if input.ClientEntryID == "" || len(input.ClientEntryID) > 128 {
		return CreateInput{}, nil, invalidField("client_entry_id", "must contain at most 128 characters")
	}
	if input.CreatedAt.IsZero() {
		return CreateInput{}, nil, invalidField("created_at", "is required")
	}
	if err := normalizeRequest(&input); err != nil {
		return CreateInput{}, nil, err
	}
	if len(input.Error) > 65536 {
		return CreateInput{}, nil, invalidField("error", "exceeds the shared history limit")
	}
	responseBody, err := decodeResponse(&input)
	if err != nil {
		return CreateInput{}, nil, err
	}
	return input, responseBody, nil
}

// normalizeRequest trims and validates the request half of a history entry and
// normalizes its body-mode-dependent fields. Header values are redacted here so
// a client bug cannot persist credentials visible to every history reader.
func normalizeRequest(input *CreateInput) error {
	input.Request.Method = strings.ToUpper(strings.TrimSpace(input.Request.Method))
	input.Request.URL = strings.TrimSpace(input.Request.URL)
	if input.Request.Method == "" || len(input.Request.Method) > 64 {
		return invalidField("request.method", "is required and must contain at most 64 characters")
	}
	if input.Request.URL == "" || len(input.Request.URL) > 16384 {
		return invalidField("request.url", "is required and must contain at most 16384 characters")
	}
	if !validBodyMode(input.Request.BodyMode) {
		return invalidField("request.body_mode", "is invalid")
	}
	if input.Request.Headers == nil {
		input.Request.Headers = []Header{}
	}
	if input.Request.BodyFields == nil {
		input.Request.BodyFields = []BodyField{}
	}
	input.Request.Headers = redactSensitiveHeaders(input.Request.Headers)
	switch input.Request.BodyMode {
	case "none":
		input.Request.Body = ""
		input.Request.BodyFields = []BodyField{}
	case "raw":
		input.Request.BodyFields = []BodyField{}
	case "form_url_encoded", "multipart_form_data":
		input.Request.Body = ""
	}
	if len(input.Request.Body) > MaxBodyBytes {
		return invalidField("request.body", "exceeds the shared history limit")
	}
	if len(input.Request.BodyLanguage) > 32 {
		return invalidField("request.raw_body_language", "must contain at most 32 characters")
	}
	if len(input.Request.Headers) > MaxHeaders || headerBytes(input.Request.Headers) > MaxHeaderBytes {
		return invalidField("request.headers", "exceed the shared history limit")
	}
	if !validHeaders(input.Request.Headers) {
		return invalidField("request.headers", "contain an invalid name or value")
	}
	if len(input.Request.BodyFields) > MaxBodyFields {
		return invalidField("request.body_fields", "exceed the shared history limit")
	}
	fieldBytes := 0
	sharedFields := make([]BodyField, 0, len(input.Request.BodyFields))
	for index := range input.Request.BodyFields {
		field := input.Request.BodyFields[index]
		if !field.Enabled {
			continue
		}
		if len(field.Name) > 4096 || len(field.Value) > MaxBodyBytes {
			return invalidField("request.body_fields", "contain an invalid name or value")
		}
		if field.Kind != "text" && field.Kind != "file" {
			return invalidField("request.body_fields", "contain an invalid kind")
		}
		if field.Kind == "file" {
			field.Value = ""
		}
		fieldBytes += len(field.Name) + len(field.Value)
		if fieldBytes > MaxBodyBytes {
			return invalidField("request.body_fields", "exceed the shared history limit")
		}
		sharedFields = append(sharedFields, field)
	}
	input.Request.BodyFields = sharedFields
	return nil
}

// decodeResponse validates the response half of a history entry and returns the
// decoded (and size-guarded) response body. Other response metadata is stored in
// the entry itself.
func decodeResponse(input *CreateInput) ([]byte, error) {
	if input.Response == nil {
		return nil, nil
	}
	if input.Response.Headers == nil {
		input.Response.Headers = []Header{}
	}
	input.Response.Headers = redactSensitiveHeaders(input.Response.Headers)
	if input.Response.Status < 100 || input.Response.Status > 999 {
		return nil, invalidField("response.status", "must be between 100 and 999")
	}
	if len(input.Response.Headers) > MaxHeaders || headerBytes(input.Response.Headers) > MaxHeaderBytes {
		return nil, invalidField("response.headers", "exceed the shared history limit")
	}
	if !validHeaders(input.Response.Headers) {
		return nil, invalidField("response.headers", "contain an invalid name or value")
	}
	if len(input.Response.StatusText) > 120 || len(input.Response.HTTPVersion) > 32 ||
		len(input.Response.FinalURL) > 16384 || len(input.Response.ContentType) > 512 {
		return nil, invalidField("response", "contains metadata that exceeds the shared history limit")
	}
	if input.Response.DurationMicros < 0 {
		return nil, invalidField("response.duration_micros", "must not be negative")
	}
	if len(input.Response.BodyBase64) > base64.StdEncoding.EncodedLen(MaxBodyBytes) {
		return nil, invalidField("response.body_base64", "exceeds the shared history limit")
	}
	decoded, err := base64.StdEncoding.DecodeString(input.Response.BodyBase64)
	if err != nil {
		return nil, invalidField("response.body_base64", "must be valid base64")
	}
	if len(decoded) > MaxBodyBytes {
		return nil, invalidField("response.body_base64", "exceeds the shared history limit")
	}
	return decoded, nil
}

func viewEntry(entry Entry) (EntryView, error) {
	var requestHeaders []Header
	if err := json.Unmarshal(entry.RequestHeadersJSON, &requestHeaders); err != nil {
		return EntryView{}, err
	}
	var requestFields []BodyField
	if err := json.Unmarshal(entry.RequestBodyFieldsJSON, &requestFields); err != nil {
		return EntryView{}, err
	}
	view := EntryView{
		ID: entry.ID, CreatedAt: entry.CreatedAt,
		Request: RequestView{
			Method: entry.Method, URL: entry.URL, Headers: requestHeaders,
			Body: string(entry.RequestBody), BodyMode: entry.RequestBodyMode,
			BodyLanguage: entry.RequestBodyLanguage, BodyFields: requestFields,
			BodyTruncated: entry.RequestBodyTruncated,
		},
		Error: entry.Error,
	}
	if entry.ResponseStatus != nil {
		var responseHeaders []Header
		if err := json.Unmarshal(entry.ResponseHeadersJSON, &responseHeaders); err != nil {
			return EntryView{}, err
		}
		duration := int64(0)
		if entry.ResponseDurationMicros != nil {
			duration = *entry.ResponseDurationMicros
		}
		view.Response = &ResponseView{
			Status: *entry.ResponseStatus, StatusText: entry.ResponseStatusText,
			HTTPVersion: entry.ResponseHTTPVersion, FinalURL: entry.ResponseFinalURL,
			Headers:       responseHeaders,
			BodyBase64:    base64.StdEncoding.EncodeToString(entry.ResponseBody),
			BodyTruncated: entry.ResponseBodyTruncated, ContentType: entry.ResponseContentType,
			DurationMicros: duration,
		}
	}
	return view, nil
}

func validBodyMode(value string) bool {
	return value == "none" || value == "raw" || value == "form_url_encoded" || value == "multipart_form_data"
}

func headerBytes(headers []Header) int {
	total := 0
	for _, header := range headers {
		total += len(header.Name) + len(header.Value)
	}
	return total
}

func validHeaders(headers []Header) bool {
	for _, header := range headers {
		if strings.TrimSpace(header.Name) == "" || len(header.Name) > 4096 || len(header.Value) > 65536 {
			return false
		}
	}
	return true
}

const redactedHeaderValue = "[REDACTED]"

// redactSensitiveHeaders replaces the values of known credential-carrying
// header names with a fixed marker. Names are matched like the desktop client's
// history redaction so both sides agree on what is sensitive.
func redactSensitiveHeaders(headers []Header) []Header {
	redacted := make([]Header, 0, len(headers))
	for _, header := range headers {
		if isSensitiveHeader(header.Name) {
			header.Value = redactedHeaderValue
		}
		redacted = append(redacted, header)
	}
	return redacted
}

func isSensitiveHeader(name string) bool {
	normalized := strings.ToLower(strings.TrimSpace(name))
	switch normalized {
	case "authorization", "proxy-authorization", "cookie", "x-api-key", "api-key",
		"x-auth-token", "set-cookie", "www-authenticate", "proxy-authenticate":
		return true
	}
	return strings.Contains(normalized, "token") ||
		strings.Contains(normalized, "secret") ||
		strings.HasSuffix(normalized, "-api-key")
}

func invalidField(field, message string) error {
	return problem.WithFields("validation_failed", "request validation failed", map[string]string{field: message})
}
