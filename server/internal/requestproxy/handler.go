package requestproxy

import (
	"resolved-server/internal/auth"
	"resolved-server/internal/httpkit"
	"resolved-server/internal/identity"
	"resolved-server/internal/requestproxy/proxybody"
	"resolved-server/internal/workspaces"

	"github.com/gofiber/fiber/v3"
)

type Handler struct {
	service *Service
}

type ExecuteRequest struct {
	WorkspaceID string         `json:"-" validate:"required"`
	Method      string         `json:"method" validate:"required,max=64"`
	URL         string         `json:"url" validate:"required,max=16384"`
	Headers     []Header       `json:"headers" validate:"max=256"`
	Body        proxybody.Body `json:"body"`
}

type ExecutePayload ExecuteRequest

type UpdateSettingsRequest struct {
	Mode              string             `json:"mode" validate:"required,oneof=local server"`
	HostnameOverrides []HostnameOverride `json:"hostname_overrides" validate:"max=256,dive"`
}

type UpdateSettingsPayload UpdateSettingsRequest

func NewHandler(service *Service) *Handler {
	return &Handler{service: service}
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
					Method:  payload.Method,
					URL:     payload.URL,
					Headers: payload.Headers,
					Body:    payload.Body,
				},
			)
			if err != nil {
				return httpkit.NewErrorResponse[ExecuteResult](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, result)
		},
	)
}

func actorFromContext(c fiber.Ctx) workspaces.Actor {
	principal := auth.PrincipalFromContext(c)
	return workspaces.Actor{
		UserID:         principal.User.ID,
		Owner:          principal.HasRole(identity.OwnerRoleID),
		EnvironmentKey: principal.EnvironmentKey(),
	}
}
