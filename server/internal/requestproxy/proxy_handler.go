package requestproxy

import (
	"resolved-server/internal/auth"
	"resolved-server/internal/httpkit"
	"resolved-server/internal/problem"

	"github.com/gofiber/fiber/v3"
)

type CreateProxyRequest struct {
	Name  string             `json:"name" validate:"required,max=120"`
	Rules []HostnameOverride `json:"rules" validate:"max=256,dive"`
}

type CreateProxyPayload CreateProxyRequest

type UpdateProxyRequest struct {
	ID    string              `json:"-" validate:"required,uuid"`
	Name  *string             `json:"name" validate:"omitempty,max=120"`
	Rules *[]HostnameOverride `json:"rules" validate:"omitempty,max=256,dive"`
}

type UpdateProxyPayload UpdateProxyRequest

type DeleteProxyRequest struct {
	ID string `json:"-" validate:"required,uuid"`
}

type DeleteProxyPayload DeleteProxyRequest

type ReplaceProxyAssignmentsRequest struct {
	ID          string            `json:"-" validate:"required,uuid"`
	Assignments []ProxyAssignment `json:"assignments" validate:"max=256"`
}

type ReplaceProxyAssignmentsPayload ReplaceProxyAssignmentsRequest

type ReplaceProxyExclusionsRequest struct {
	ID              string   `json:"-" validate:"required,uuid"`
	ExcludedUserIDs []string `json:"excluded_user_ids" validate:"max=256"`
	ExcludedRoleIDs []string `json:"excluded_role_ids" validate:"max=256"`
}

type ReplaceProxyExclusionsPayload ReplaceProxyExclusionsRequest

func (request *CreateProxyRequest) BindFiber(c fiber.Ctx) error {
	return c.Bind().Body(request)
}

func (request *CreateProxyRequest) ToPayload(*httpkit.ProcessingContext) (CreateProxyPayload, error) {
	return CreateProxyPayload(*request), nil
}

func (request *CreateProxyRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *UpdateProxyRequest) BindFiber(c fiber.Ctx) error {
	request.ID = c.Params("proxy_id")
	return c.Bind().Body(request)
}

func (request *UpdateProxyRequest) ToPayload(*httpkit.ProcessingContext) (UpdateProxyPayload, error) {
	if request.Name == nil && request.Rules == nil {
		return UpdateProxyPayload{}, problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"body": "must contain at least one change"},
		)
	}
	return UpdateProxyPayload(*request), nil
}

func (request *UpdateProxyRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *DeleteProxyRequest) BindFiber(c fiber.Ctx) error {
	request.ID = c.Params("proxy_id")
	return nil
}

func (request *DeleteProxyRequest) ToPayload(*httpkit.ProcessingContext) (DeleteProxyPayload, error) {
	return DeleteProxyPayload(*request), nil
}

func (request *DeleteProxyRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *ReplaceProxyAssignmentsRequest) BindFiber(c fiber.Ctx) error {
	request.ID = c.Params("proxy_id")
	return c.Bind().Body(request)
}

func (request *ReplaceProxyAssignmentsRequest) ToPayload(*httpkit.ProcessingContext) (ReplaceProxyAssignmentsPayload, error) {
	return ReplaceProxyAssignmentsPayload(*request), nil
}

func (request *ReplaceProxyAssignmentsRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *ReplaceProxyExclusionsRequest) BindFiber(c fiber.Ctx) error {
	request.ID = c.Params("proxy_id")
	return c.Bind().Body(request)
}

func (request *ReplaceProxyExclusionsRequest) ToPayload(*httpkit.ProcessingContext) (ReplaceProxyExclusionsPayload, error) {
	return ReplaceProxyExclusionsPayload(*request), nil
}

func (request *ReplaceProxyExclusionsRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (h *Handler) ListProxiesController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		[]Proxy,
		httpkit.EmptyPayload,
		httpkit.EmptyRequest,
	](
		func(c fiber.Ctx, _ httpkit.EmptyPayload) httpkit.Response[[]Proxy] {
			proxies, err := h.service.ListProxies(c.Context())
			if err != nil {
				return httpkit.NewErrorResponse[[]Proxy](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, proxies)
		},
	)
}

func (h *Handler) CreateProxyController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		Proxy,
		CreateProxyPayload,
		CreateProxyRequest,
	](
		func(c fiber.Ctx, payload CreateProxyPayload) httpkit.Response[Proxy] {
			principal := auth.PrincipalFromContext(c)
			proxy, err := h.service.CreateProxy(c.Context(), principal.User.ID, payload.Name, payload.Rules)
			if err != nil {
				return httpkit.NewErrorResponse[Proxy](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusCreated, proxy)
		},
	)
}

func (h *Handler) UpdateProxyController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		Proxy,
		UpdateProxyPayload,
		UpdateProxyRequest,
	](
		func(c fiber.Ctx, payload UpdateProxyPayload) httpkit.Response[Proxy] {
			principal := auth.PrincipalFromContext(c)
			proxy, err := h.service.UpdateProxy(c.Context(), principal.User.ID, payload.ID, payload.Name, payload.Rules)
			if err != nil {
				return httpkit.NewErrorResponse[Proxy](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, proxy)
		},
	)
}

func (h *Handler) DeleteProxyController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		Proxy,
		DeleteProxyPayload,
		DeleteProxyRequest,
	](
		func(c fiber.Ctx, payload DeleteProxyPayload) httpkit.Response[Proxy] {
			principal := auth.PrincipalFromContext(c)
			proxy, err := h.service.DeleteProxy(c.Context(), principal.User.ID, payload.ID)
			if err != nil {
				return httpkit.NewErrorResponse[Proxy](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, proxy)
		},
	)
}

func (h *Handler) ReplaceProxyAssignmentsController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		Proxy,
		ReplaceProxyAssignmentsPayload,
		ReplaceProxyAssignmentsRequest,
	](
		func(c fiber.Ctx, payload ReplaceProxyAssignmentsPayload) httpkit.Response[Proxy] {
			principal := auth.PrincipalFromContext(c)
			proxy, err := h.service.ReplaceProxyAssignments(
				c.Context(), principal.User.ID, payload.ID, payload.Assignments,
			)
			if err != nil {
				return httpkit.NewErrorResponse[Proxy](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, proxy)
		},
	)
}

func (h *Handler) ReplaceProxyExclusionsController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		Proxy,
		ReplaceProxyExclusionsPayload,
		ReplaceProxyExclusionsRequest,
	](
		func(c fiber.Ctx, payload ReplaceProxyExclusionsPayload) httpkit.Response[Proxy] {
			principal := auth.PrincipalFromContext(c)
			proxy, err := h.service.ReplaceProxyExclusions(
				c.Context(), principal.User.ID, payload.ID, payload.ExcludedUserIDs, payload.ExcludedRoleIDs,
			)
			if err != nil {
				return httpkit.NewErrorResponse[Proxy](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, proxy)
		},
	)
}
