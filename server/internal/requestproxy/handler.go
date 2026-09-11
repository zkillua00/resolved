package requestproxy

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"log"
	"net/http"
	"time"

	"resolved-server/internal/auth"
	"resolved-server/internal/httpkit"
	"resolved-server/internal/identity"
	"resolved-server/internal/problem"
	"resolved-server/internal/requestproxy/proxybody"
	"resolved-server/internal/workspaces"

	"github.com/gofiber/fiber/v3"
	"github.com/gofiber/fiber/v3/middleware/adaptor"
	gorillaWebsocket "github.com/gorilla/websocket"
)

type Handler struct {
	service        *Service
	websocketSlots chan struct{}
}

const (
	maxWebSocketExecutions      = 500
	maxWebSocketDescriptorBytes = 1024 * 1024
	maxWebSocketMessageBytes    = 16 * 1024 * 1024
)

type websocketExecutionContextKey struct{}

type websocketExecutionContext struct {
	actor       workspaces.Actor
	workspaceID string
}

type websocketOpenResponse struct {
	Type        string `json:"type"`
	Message     string `json:"message,omitempty"`
	Subprotocol string `json:"subprotocol,omitempty"`
}

type ExecuteRequest struct {
	UseCookieJar bool           `json:"use_cookie_jar"`
	WorkspaceID  string         `json:"-" validate:"required"`
	RequestID    string         `json:"request_id" validate:"omitempty,uuid"`
	Method       string         `json:"method" validate:"required,max=64"`
	URL          string         `json:"url" validate:"required,max=16384"`
	Headers      []Header       `json:"headers" validate:"max=256"`
	Body         proxybody.Body `json:"body"`
}

type ExecutePayload ExecuteRequest

type UpdateSettingsRequest struct {
	Mode string `json:"mode" validate:"required,oneof=local server"`
}

type UpdateSettingsPayload UpdateSettingsRequest

type AddAllowlistEntryRequest struct {
	Kind  string `json:"kind" validate:"required,oneof=request address"`
	Value string `json:"value" validate:"required,max=16384"`
}

type AddAllowlistEntryPayload AddAllowlistEntryRequest

func NewHandler(service *Service) *Handler {
	return &Handler{
		service:        service,
		websocketSlots: make(chan struct{}, maxWebSocketExecutions),
	}
}

func (request *ExecuteRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	return c.Bind().Body(request)
}

func (request *ExecuteRequest) ToPayload(*httpkit.ProcessingContext) (ExecutePayload, error) {
	return ExecutePayload(*request), nil
}

func (request *ExecuteRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *UpdateSettingsRequest) BindFiber(c fiber.Ctx) error {
	return c.Bind().Body(request)
}

func (request *UpdateSettingsRequest) ToPayload(*httpkit.ProcessingContext) (UpdateSettingsPayload, error) {
	return UpdateSettingsPayload(*request), nil
}

func (request *UpdateSettingsRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *AddAllowlistEntryRequest) BindFiber(c fiber.Ctx) error {
	return c.Bind().Body(request)
}

func (request *AddAllowlistEntryRequest) ToPayload(*httpkit.ProcessingContext) (AddAllowlistEntryPayload, error) {
	return AddAllowlistEntryPayload(*request), nil
}

func (request *AddAllowlistEntryRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (h *Handler) PolicyController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		Policy,
		httpkit.EmptyPayload,
		httpkit.EmptyRequest,
	](
		func(c fiber.Ctx, _ httpkit.EmptyPayload) httpkit.Response[Policy] {
			policy, err := h.service.Policy(c.Context())
			if err != nil {
				return httpkit.NewErrorResponse[Policy](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, policy)
		},
	)
}

func (h *Handler) SettingsController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		Settings,
		httpkit.EmptyPayload,
		httpkit.EmptyRequest,
	](
		func(c fiber.Ctx, _ httpkit.EmptyPayload) httpkit.Response[Settings] {
			settings, err := h.service.Settings(c.Context())
			if err != nil {
				return httpkit.NewErrorResponse[Settings](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, settings)
		},
	)
}

func (h *Handler) UpdateSettingsController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		Settings,
		UpdateSettingsPayload,
		UpdateSettingsRequest,
	](
		func(c fiber.Ctx, payload UpdateSettingsPayload) httpkit.Response[Settings] {
			principal := auth.PrincipalFromContext(c)
			settings, err := h.service.UpdateSettings(
				c.Context(),
				principal.User.ID,
				Settings{Mode: payload.Mode},
			)
			if err != nil {
				return httpkit.NewErrorResponse[Settings](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, settings)
		},
	)
}

func (h *Handler) ExecuteController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		ExecuteResult,
		ExecutePayload,
		ExecuteRequest,
	](
		func(c fiber.Ctx, payload ExecutePayload) httpkit.Response[ExecuteResult] {
			result, err := h.service.Execute(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				ExecuteInput{
					UseCookieJar: payload.UseCookieJar,
					RequestID:    payload.RequestID,
					Method:       payload.Method,
					URL:          payload.URL,
					Headers:      payload.Headers,
					Body:         payload.Body,
				},
			)
			if err != nil {
				return httpkit.NewErrorResponse[ExecuteResult](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, result)
		},
	)
}

func (h *Handler) WebSocketController() fiber.Handler {
	handler := adaptor.HTTPHandlerWithContext(http.HandlerFunc(h.handleWebSocket))
	return func(c fiber.Ctx) error {
		select {
		case h.websocketSlots <- struct{}{}:
			defer func() { <-h.websocketSlots }()
		default:
			return fiber.ErrTooManyRequests
		}
		ctx := context.WithValue(c.Context(), websocketExecutionContextKey{}, websocketExecutionContext{
			actor:       actorFromContext(c),
			workspaceID: c.Params("workspace_id"),
		})
		c.SetContext(ctx)
		return handler(c)
	}
}

func (h *Handler) handleWebSocket(w http.ResponseWriter, r *http.Request) {
	outer, err := (&gorillaWebsocket.Upgrader{
		HandshakeTimeout: 5 * time.Second,
		CheckOrigin:      func(*http.Request) bool { return true },
	}).Upgrade(w, r, nil)
	if err != nil {
		return
	}
	defer outer.Close()
	outer.SetReadLimit(maxWebSocketDescriptorBytes)

	messageType, payload, err := outer.ReadMessage()
	if err != nil || messageType != gorillaWebsocket.TextMessage {
		_ = outer.WriteJSON(websocketOpenResponse{Type: "error", Message: "the first frame must be a WebSocket execution descriptor"})
		return
	}
	var input WebSocketOpenInput
	if err := json.Unmarshal(payload, &input); err != nil {
		_ = outer.WriteJSON(websocketOpenResponse{Type: "error", Message: "the WebSocket execution descriptor is invalid"})
		return
	}
	fiberContext, contextOK := adaptor.LocalContextFromHTTPRequest(r)
	if !contextOK {
		fiberContext = r.Context()
	}
	route, ok := fiberContext.Value(websocketExecutionContextKey{}).(websocketExecutionContext)
	if !ok {
		_ = outer.WriteJSON(websocketOpenResponse{Type: "error", Message: "the WebSocket execution context is unavailable"})
		return
	}
	startedAt := time.Now()
	upstream, target, err := h.service.OpenWebSocket(fiberContext, route.actor, route.workspaceID, input)
	if err != nil {
		writeWebSocketOpenError(outer, err)
		return
	}
	defer upstream.Close()
	outer.SetReadLimit(maxWebSocketMessageBytes)
	upstream.SetReadLimit(maxWebSocketMessageBytes)
	defer func() {
		h.service.recordExecution(
			fiberContext, route.actor, route.workspaceID, "WEBSOCKET", target, 101, time.Since(startedAt),
		)
	}()
	if err := outer.WriteJSON(websocketOpenResponse{Type: "opened", Subprotocol: upstream.Subprotocol()}); err != nil {
		return
	}

	errors := make(chan error, 2)
	bridgeWebSocketControlFrames(outer, upstream)
	bridgeWebSocketControlFrames(upstream, outer)
	go relayWebSocket(outer, upstream, errors)
	go relayWebSocket(upstream, outer, errors)
	<-errors
}

func writeWebSocketOpenError(connection *gorillaWebsocket.Conn, err error) {
	var requestError *problem.Error
	if !errors.As(err, &requestError) || requestError.Kind == problem.KindInternal {
		log.Printf("open proxied websocket: %v", err)
		_ = connection.WriteJSON(websocketOpenResponse{Type: "error", Message: "the server could not open the WebSocket connection"})
		return
	}
	_ = connection.WriteJSON(websocketOpenResponse{Type: "error", Message: requestError.Message})
}

func bridgeWebSocketControlFrames(source, destination *gorillaWebsocket.Conn) {
	write := func(messageType int, payload []byte) error {
		return destination.WriteControl(messageType, payload, time.Now().Add(5*time.Second))
	}
	source.SetPingHandler(func(payload string) error {
		return write(gorillaWebsocket.PingMessage, []byte(payload))
	})
	source.SetPongHandler(func(payload string) error {
		return write(gorillaWebsocket.PongMessage, []byte(payload))
	})
	source.SetCloseHandler(func(code int, text string) error {
		return write(gorillaWebsocket.CloseMessage, gorillaWebsocket.FormatCloseMessage(code, text))
	})
}

func relayWebSocket(source, destination *gorillaWebsocket.Conn, errors chan<- error) {
	for {
		messageType, payload, err := source.ReadMessage()
		if err != nil {
			errors <- err
			return
		}
		if err := destination.WriteMessage(messageType, payload); err != nil {
			errors <- err
			return
		}
	}
}

func (h *Handler) AddAllowlistEntryController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		AllowlistEntry,
		AddAllowlistEntryPayload,
		AddAllowlistEntryRequest,
	](
		func(c fiber.Ctx, payload AddAllowlistEntryPayload) httpkit.Response[AllowlistEntry] {
			principal := auth.PrincipalFromContext(c)
			entry, err := h.service.AddAllowlistEntry(c.Context(), principal.User.ID, AllowlistEntry{
				Kind: payload.Kind, Value: payload.Value,
			})
			if err != nil {
				return httpkit.NewErrorResponse[AllowlistEntry](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, entry)
		},
	)
}

func actorFromContext(c fiber.Ctx) workspaces.Actor {
	principal := auth.PrincipalFromContext(c)
	roleIDs := make([]string, 0, len(principal.User.Roles))
	for _, role := range principal.User.Roles {
		roleIDs = append(roleIDs, role.ID)
	}
	return workspaces.Actor{
		UserID:            principal.User.ID,
		Owner:             principal.HasRole(identity.OwnerRoleID),
		RoleIDs:           roleIDs,
		EnvironmentKey:    principal.EnvironmentKey(),
		CredentialVersion: sha256.Sum256([]byte(principal.User.PasswordHash)),
	}
}
