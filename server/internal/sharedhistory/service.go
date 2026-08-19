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

func (s *Service) ListProfiles(ctx context.Context) ([]ProfileView, error) {
	users, err := s.repository.ListProfiles(ctx)
	if err != nil {
		return nil, problem.Wrap(err, "list profiles")
	}
	profiles := make([]ProfileView, 0, len(users))
	for _, user := range users {
		profiles = append(profiles, ProfileView{
			ID: user.ID, Email: user.Email, DisplayName: user.DisplayName, Active: user.Active,
		})
	}
	return profiles, nil
}

func (s *Service) List(
	ctx context.Context,
	actor workspaces.Actor,
	canReadOthers bool,
	workspaceID, userID string,
) ([]EntryView, error) {
	if err := validateUUID("workspace_id", workspaceID); err != nil {
		return nil, err
	}
	if err := validateUUID("user_id", userID); err != nil {
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
	if err := validateUUID("workspace_id", workspaceID); err != nil {
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
	if err := validateUUID("workspace_id", workspaceID); err != nil {
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
	input.Request.Method = strings.ToUpper(strings.TrimSpace(input.Request.Method))
	input.Request.URL = strings.TrimSpace(input.Request.URL)
	if input.ClientEntryID == "" || len(input.ClientEntryID) > 128 {
		return CreateInput{}, nil, invalidField("client_entry_id", "must contain at most 128 characters")
	}
	if input.CreatedAt.IsZero() {
		return CreateInput{}, nil, invalidField("created_at", "is required")
	}
	if input.Request.Method == "" || len(input.Request.Method) > 64 {
		return CreateInput{}, nil, invalidField("request.method", "is required and must contain at most 64 characters")
	}
	if input.Request.URL == "" || len(input.Request.URL) > 16384 {
		return CreateInput{}, nil, invalidField("request.url", "is required and must contain at most 16384 characters")
	}
	if !validBodyMode(input.Request.BodyMode) {
		return CreateInput{}, nil, invalidField("request.body_mode", "is invalid")
	}
	if input.Request.Headers == nil {
		input.Request.Headers = []Header{}
	}
	if input.Request.BodyFields == nil {
		input.Request.BodyFields = []BodyField{}
	}
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
		return CreateInput{}, nil, invalidField("request.body", "exceeds the shared history limit")
	}
	if len(input.Request.BodyLanguage) > 32 {
		return CreateInput{}, nil, invalidField("request.raw_body_language", "must contain at most 32 characters")
	}
	if len(input.Request.Headers) > MaxHeaders || headerBytes(input.Request.Headers) > MaxHeaderBytes {
		return CreateInput{}, nil, invalidField("request.headers", "exceed the shared history limit")
	}
	if !validHeaders(input.Request.Headers) {
		return CreateInput{}, nil, invalidField("request.headers", "contain an invalid name or value")
	}
	if len(input.Request.BodyFields) > MaxBodyFields {
		return CreateInput{}, nil, invalidField("request.body_fields", "exceed the shared history limit")
	}
	fieldBytes := 0
	sharedFields := make([]BodyField, 0, len(input.Request.BodyFields))
	for index := range input.Request.BodyFields {
		field := input.Request.BodyFields[index]
		if !field.Enabled {
			continue
		}
		if len(field.Name) > 4096 || len(field.Value) > MaxBodyBytes {
			return CreateInput{}, nil, invalidField("request.body_fields", "contain an invalid name or value")
		}
		if field.Kind != "text" && field.Kind != "file" {
			return CreateInput{}, nil, invalidField("request.body_fields", "contain an invalid kind")
		}
		if field.Kind == "file" {
			field.Value = ""
		}
		fieldBytes += len(field.Name) + len(field.Value)
		if fieldBytes > MaxBodyBytes {
			return CreateInput{}, nil, invalidField("request.body_fields", "exceed the shared history limit")
		}
		sharedFields = append(sharedFields, field)
	}
	input.Request.BodyFields = sharedFields
	if len(input.Error) > 65536 {
		return CreateInput{}, nil, invalidField("error", "exceeds the shared history limit")
	}

	var responseBody []byte
	if input.Response != nil {
		if input.Response.Headers == nil {
			input.Response.Headers = []Header{}
		}
		if input.Response.Status < 100 || input.Response.Status > 999 {
			return CreateInput{}, nil, invalidField("response.status", "must be between 100 and 999")
		}
		if len(input.Response.Headers) > MaxHeaders || headerBytes(input.Response.Headers) > MaxHeaderBytes {
			return CreateInput{}, nil, invalidField("response.headers", "exceed the shared history limit")
		}
		if !validHeaders(input.Response.Headers) {
			return CreateInput{}, nil, invalidField("response.headers", "contain an invalid name or value")
		}
		if len(input.Response.StatusText) > 120 || len(input.Response.HTTPVersion) > 32 ||
			len(input.Response.FinalURL) > 16384 || len(input.Response.ContentType) > 512 {
			return CreateInput{}, nil, invalidField("response", "contains metadata that exceeds the shared history limit")
		}
		if input.Response.DurationMicros < 0 {
			return CreateInput{}, nil, invalidField("response.duration_micros", "must not be negative")
		}
		if len(input.Response.BodyBase64) > base64.StdEncoding.EncodedLen(MaxBodyBytes) {
			return CreateInput{}, nil, invalidField("response.body_base64", "exceeds the shared history limit")
		}
		decoded, err := base64.StdEncoding.DecodeString(input.Response.BodyBase64)
		if err != nil {
			return CreateInput{}, nil, invalidField("response.body_base64", "must be valid base64")
		}
		if len(decoded) > MaxBodyBytes {
			return CreateInput{}, nil, invalidField("response.body_base64", "exceeds the shared history limit")
		}
		responseBody = decoded
	}
	return input, responseBody, nil
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

func validateUUID(field, value string) error {
	if _, err := uuid.Parse(value); err != nil {
		return invalidField(field, "must be a valid UUID")
	}
	return nil
}

func invalidField(field, message string) error {
	return problem.WithFields("validation_failed", "request validation failed", map[string]string{field: message})
}
