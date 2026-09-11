package requestproxy

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"io"
	"log"
	"net/http"
	"sync"
	"sync/atomic"
	"time"

	"resolved-server/internal/auth"
	"resolved-server/internal/executionlimits"
	"resolved-server/internal/httpkit"
	"resolved-server/internal/identity"
	"resolved-server/internal/problem"
	"resolved-server/internal/requestproxy/proxybody"
	"resolved-server/internal/workspaces"

	"github.com/gofiber/fiber/v3"
	"github.com/gofiber/fiber/v3/middleware/adaptor"
	gorillaWebsocket "github.com/gorilla/websocket"
	"github.com/valyala/fasthttp"
)

type Handler struct {
	service        *Service
	slotsMu        sync.Mutex
	websocketSlots map[string]int64
}

const (
	maxWebSocketMessageBytes = 16 * 1024 * 1024
)

type websocketExecutionContextKey struct{}

type websocketExecutionContext struct {
	actor        workspaces.Actor
	workspaceID  string
	collectionID string
	snapshot     executionlimits.Snapshot
}

type websocketOpenResponse struct {
	Type        string `json:"type"`
	Message     string `json:"message,omitempty"`
	Subprotocol string `json:"subprotocol,omitempty"`
}

type ExecuteRequest struct {
	CollectionID string         `json:"collection_id,omitempty"`
	UseCookieJar bool           `json:"use_cookie_jar"`
	WorkspaceID  string         `json:"-" validate:"required"`
	Method       string         `json:"method" validate:"required,max=64"`
	URL          string         `json:"url" validate:"required"`
	Headers      []Header       `json:"headers"`
	Body         proxybody.Body `json:"body"`
}

type ExecutePayload ExecuteRequest

type UpdateSettingsRequest struct {
	Mode              string             `json:"mode" validate:"required,oneof=local server"`
	HostnameOverrides []HostnameOverride `json:"hostname_overrides" validate:"max=256,dive"`
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
		websocketSlots: make(map[string]int64),
	}
}

func (request *ExecuteRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	snapshot, ok := c.Context().Value(executionSnapshotKey{}).(executionSnapshot)
	if !ok {
		return fiber.ErrInternalServerError
	}
	var reader io.Reader = c.Request().BodyStream()
	if reader == nil {
		reader = bytes.NewReader(c.Request().Body())
	}
	body, tooLarge, err := readLimited(reader, snapshot.snapshot.Effective["http.envelope_bytes"])
	if err != nil {
		return err
	}
	if tooLarge {
		return fiber.NewError(fiber.StatusRequestEntityTooLarge, "execution envelope exceeds http.envelope_bytes")
	}
	// Preserve Fiber's supported Content-Encoding behavior without allowing
	// decompression to bypass the effective envelope budget. Bound both the wire
	// bytes above and the decoded JSON bytes here.
	c.Request().SetBodyRaw(body)
	bound := snapshot.snapshot.Effective["http.envelope_bytes"]
	maxDecoded := 0
	if !bound.Unlimited {
		maxDecoded = int(bound.Value)
	}
	body, err = c.Request().BodyUncompressedWithLimit(maxDecoded)
	if errors.Is(err, fasthttp.ErrBodyTooLarge) {
		return fiber.NewError(fiber.StatusRequestEntityTooLarge, "decoded execution envelope exceeds http.envelope_bytes")
	}
	if errors.Is(err, fasthttp.ErrContentEncodingUnsupported) {
		return fiber.ErrUnsupportedMediaType
	}
	if err != nil {
		return fiber.ErrBadRequest
	}
	if err := json.Unmarshal(body, request); err != nil {
		return fiber.ErrBadRequest
	}
	if request.CollectionID != "" && request.CollectionID != snapshot.collectionID {
		return invalidField("collection_id", "must match the query scope")
	}
	request.CollectionID = snapshot.collectionID
	return nil
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
			snapshot, err := h.service.resolveLimits(c.Context(), actorFromContext(c), c.Query("workspace_id"), c.Query("collection_id"))
			if err != nil {
				return httpkit.NewErrorResponse[Policy](err)
			}
			ctx := context.WithValue(c.Context(), executionSnapshotKey{}, executionSnapshot{c.Query("workspace_id"), c.Query("collection_id"), snapshot})
			policy, err := h.service.Policy(ctx)
			if err != nil {
				return httpkit.NewErrorResponse[Policy](err)
			}
			policy.Limits = snapshot.Effective
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
				Settings{
					Mode:              payload.Mode,
					HostnameOverrides: payload.HostnameOverrides,
				},
			)
			if err != nil {
				return httpkit.NewErrorResponse[Settings](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, settings)
		},
	)
}

func (h *Handler) ExecuteController() fiber.Handler {
	handler := httpkit.WithProcessedPayload[
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
					CollectionID: payload.CollectionID,
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
	return func(c fiber.Ctx) error {
		workspaceID, collectionID := c.Params("workspace_id"), c.Query("collection_id")
		snapshot, err := h.service.resolveLimits(c.Context(), actorFromContext(c), workspaceID, collectionID)
		if err != nil {
			return err
		}
		c.SetContext(context.WithValue(c.Context(), executionSnapshotKey{}, executionSnapshot{workspaceID, collectionID, snapshot}))
		return handler(c)
	}
}

func (h *Handler) WebSocketController() fiber.Handler {
	return func(c fiber.Ctx) error {
		workspaceID, collectionID := c.Params("workspace_id"), c.Query("collection_id")
		snapshot, err := h.service.resolveLimits(c.Context(), actorFromContext(c), workspaceID, collectionID)
		if err != nil {
			return err
		}
		release, admitted := h.admitWebSocket(workspaceID+"\x00"+collectionID, snapshot.Effective["websocket.concurrent_sessions"])
		if !admitted {
			return fiber.ErrTooManyRequests
		}
		// The adaptor returns to Fiber as soon as Upgrade hijacks the socket;
		// the net/http handler continues in another goroutine. That handler owns
		// the permit for the entire connection, not just the upgrade call.
		var handedOff atomic.Bool
		defer func() {
			if !handedOff.Load() {
				release()
			}
		}()
		c.SetContext(context.WithValue(c.Context(), executionSnapshotKey{}, executionSnapshot{workspaceID, collectionID, snapshot}))
		ctx := context.WithValue(c.Context(), websocketExecutionContextKey{}, websocketExecutionContext{
			actor:        actorFromContext(c),
			workspaceID:  workspaceID,
			collectionID: collectionID,
			snapshot:     snapshot,
		})
		c.SetContext(ctx)
		return adaptor.HTTPHandlerWithContext(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			handedOff.Store(true)
			defer release()
			h.handleWebSocket(w, r)
		}))(c)
	}
}

func (h *Handler) handleWebSocket(w http.ResponseWriter, r *http.Request) {
	fiberContext, contextOK := adaptor.LocalContextFromHTTPRequest(r)
	if !contextOK {
		fiberContext = r.Context()
	}
	route, ok := fiberContext.Value(websocketExecutionContextKey{}).(websocketExecutionContext)
	if !ok {
		http.Error(w, "execution context unavailable", http.StatusInternalServerError)
		return
	}
	outer, err := (&gorillaWebsocket.Upgrader{
		HandshakeTimeout: limitDuration(route.snapshot.Effective["websocket.handshake_timeout_ms"]),
		CheckOrigin:      func(*http.Request) bool { return true },
	}).Upgrade(w, r, nil)
	if err != nil {
		return
	}
	defer outer.Close()
	setWebSocketReadLimit(outer, route.snapshot.Effective["websocket.opening_bytes"])
	if duration := limitDuration(route.snapshot.Effective["websocket.handshake_timeout_ms"]); duration != 0 {
		_ = outer.SetReadDeadline(time.Now().Add(duration))
	}

	messageType, payload, err := outer.ReadMessage()
	_ = outer.SetReadDeadline(time.Time{})
	if exceeds(route.snapshot.Effective["websocket.opening_bytes"], int64(len(payload))) {
		return
	}
	if err != nil || messageType != gorillaWebsocket.TextMessage {
		_ = outer.WriteJSON(websocketOpenResponse{Type: "error", Message: "the first frame must be a WebSocket execution descriptor"})
		return
	}
	var input WebSocketOpenInput
	if err := json.Unmarshal(payload, &input); err != nil {
		_ = outer.WriteJSON(websocketOpenResponse{Type: "error", Message: "the WebSocket execution descriptor is invalid"})
		return
	}
	if input.CollectionID != "" && input.CollectionID != route.collectionID {
		_ = outer.WriteJSON(websocketOpenResponse{Type: "error", Message: "collection_id must match the query scope"})
		return
	}
	input.CollectionID = route.collectionID
	startedAt := time.Now()
	upstream, target, err := h.service.OpenWebSocket(fiberContext, route.actor, route.workspaceID, input)
	if err != nil {
		writeWebSocketOpenError(outer, err)
		return
	}
	defer upstream.Close()
	messageBound := route.snapshot.Effective["websocket.message_bytes"]
	setWebSocketReadLimit(outer, messageBound)
	setWebSocketReadLimit(upstream, messageBound)
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
	go relayWebSocketBounded(outer, upstream, errors, messageBound)
	go relayWebSocketBounded(upstream, outer, errors, messageBound)
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
	relayWebSocketBounded(source, destination, errors, executionlimits.Bound{Value: maxWebSocketMessageBytes})
}

func relayWebSocketBounded(source, destination *gorillaWebsocket.Conn, errors chan<- error, bound executionlimits.Bound) {
	for {
		messageType, payload, err := source.ReadMessage()
		if err != nil {
			errors <- err
			return
		}
		if exceeds(bound, int64(len(payload))) {
			errors <- fiber.ErrRequestEntityTooLarge
			return
		}
		if err := destination.WriteMessage(messageType, payload); err != nil {
			errors <- err
			return
		}
	}
}

func setWebSocketReadLimit(connection *gorillaWebsocket.Conn, bound executionlimits.Bound) {
	if bound.Unlimited {
		connection.SetReadLimit(0)
		return
	}
	// Gorilla interprets zero as unlimited; permit at most one byte and reject
	// nonempty messages explicitly when the configured bound is zero.
	value := bound.Value
	if value == 0 {
		value = 1
	}
	connection.SetReadLimit(value)
}

func (h *Handler) admitWebSocket(scope string, bound executionlimits.Bound) (func(), bool) {
	h.slotsMu.Lock()
	defer h.slotsMu.Unlock()
	if !bound.Unlimited && h.websocketSlots[scope] >= bound.Value {
		return nil, false
	}
	h.websocketSlots[scope]++
	var once sync.Once
	return func() {
		once.Do(func() {
			h.slotsMu.Lock()
			defer h.slotsMu.Unlock()
			h.websocketSlots[scope]--
			if h.websocketSlots[scope] == 0 {
				delete(h.websocketSlots, scope)
			}
		})
	}, true
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
	return workspaces.Actor{
		UserID:            principal.User.ID,
		Owner:             principal.HasRole(identity.OwnerRoleID),
		EnvironmentKey:    principal.EnvironmentKey(),
		CredentialVersion: sha256.Sum256([]byte(principal.User.PasswordHash)),
	}
}
