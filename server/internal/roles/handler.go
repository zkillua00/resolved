package roles

import (
	"resolved-server/internal/auth"
	"resolved-server/internal/httpkit"
	"resolved-server/internal/identity"
	"resolved-server/internal/problem"

	"github.com/gofiber/fiber/v3"
)

type Handler struct {
	service *Service
}

type CreateRequest struct {
	Name           string   `json:"name" validate:"required,min=2,max=100"`
	Description    string   `json:"description" validate:"max=500"`
	PermissionKeys []string `json:"permission_keys" validate:"dive,required,max=100"`
}

type CreatePayload CreateRequest

type GetRequest struct {
	ID string `json:"-" validate:"required"`
}

type GetPayload GetRequest

type UpdateRequest struct {
	ID          string  `json:"-" validate:"required"`
	Name        *string `json:"name" validate:"omitempty,min=2,max=100"`
	Description *string `json:"description" validate:"omitempty,max=500"`
}

type UpdatePayload UpdateRequest

type ReplacePermissionsRequest struct {
	ID             string   `json:"-" validate:"required"`
	PermissionKeys []string `json:"permission_keys" validate:"dive,required,max=100"`
}

type ReplacePermissionsPayload ReplacePermissionsRequest

func NewHandler(service *Service) *Handler {
	return &Handler{service: service}
}

func (request *CreateRequest) BindFiber(c fiber.Ctx) error {
	return c.Bind().Body(request)
}

func (request *CreateRequest) ToPayload(*httpkit.ProcessingContext) (CreatePayload, error) {
	return CreatePayload(*request), nil
}

func (request *CreateRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *GetRequest) BindFiber(c fiber.Ctx) error {
	request.ID = c.Params("id")
	return nil
}

func (request *GetRequest) ToPayload(*httpkit.ProcessingContext) (GetPayload, error) {
	return GetPayload(*request), nil
}

func (request *GetRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *UpdateRequest) BindFiber(c fiber.Ctx) error {
	request.ID = c.Params("id")
	return c.Bind().Body(request)
}

func (request *UpdateRequest) ToPayload(*httpkit.ProcessingContext) (UpdatePayload, error) {
	if request.Name == nil && request.Description == nil {
		return UpdatePayload{}, problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"body": "must contain at least one change"},
		)
	}
	return UpdatePayload(*request), nil
}

func (request *UpdateRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *ReplacePermissionsRequest) BindFiber(c fiber.Ctx) error {
	request.ID = c.Params("id")
	return c.Bind().Body(request)
}

func (request *ReplacePermissionsRequest) ToPayload(*httpkit.ProcessingContext) (ReplacePermissionsPayload, error) {
	return ReplacePermissionsPayload(*request), nil
}

func (request *ReplacePermissionsRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (h *Handler) ListController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		[]identity.RoleView,
		httpkit.EmptyPayload,
		httpkit.EmptyRequest,
	](
		func(c fiber.Ctx, _ httpkit.EmptyPayload) httpkit.Response[[]identity.RoleView] {
			result, err := h.service.List(c.Context())
			if err != nil {
				return httpkit.NewErrorResponse[[]identity.RoleView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, identity.ViewRoles(result))
		},
	)
}

func (h *Handler) CreateController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		identity.RoleView,
		CreatePayload,
		CreateRequest,
	](
		func(c fiber.Ctx, payload CreatePayload) httpkit.Response[identity.RoleView] {
			creatorID := auth.PrincipalFromContext(c).User.ID
			result, err := h.service.Create(c.Context(), CreateInput{
				Name:            payload.Name,
				Description:     payload.Description,
				PermissionKeys:  payload.PermissionKeys,
				CreatedByUserID: &creatorID,
			})
			if err != nil {
				return httpkit.NewErrorResponse[identity.RoleView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusCreated, identity.ViewRole(result))
		},
	)
}

func (h *Handler) GetController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		identity.RoleView,
		GetPayload,
		GetRequest,
	](
		func(c fiber.Ctx, payload GetPayload) httpkit.Response[identity.RoleView] {
			result, err := h.service.Get(c.Context(), payload.ID)
			if err != nil {
				return httpkit.NewErrorResponse[identity.RoleView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, identity.ViewRole(result))
		},
	)
}

func (h *Handler) UpdateController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		identity.RoleView,
		UpdatePayload,
		UpdateRequest,
	](
		func(c fiber.Ctx, payload UpdatePayload) httpkit.Response[identity.RoleView] {
			result, err := h.service.Update(c.Context(), payload.ID, UpdateInput{
				Name:        payload.Name,
				Description: payload.Description,
			})
			if err != nil {
				return httpkit.NewErrorResponse[identity.RoleView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, identity.ViewRole(result))
		},
	)
}

func (h *Handler) ReplacePermissionsController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		identity.RoleView,
		ReplacePermissionsPayload,
		ReplacePermissionsRequest,
	](
		func(c fiber.Ctx, payload ReplacePermissionsPayload) httpkit.Response[identity.RoleView] {
			result, err := h.service.ReplacePermissions(c.Context(), payload.ID, payload.PermissionKeys)
			if err != nil {
				return httpkit.NewErrorResponse[identity.RoleView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, identity.ViewRole(result))
		},
	)
}

func (h *Handler) ListPermissionsController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		[]identity.PermissionView,
		httpkit.EmptyPayload,
		httpkit.EmptyRequest,
	](
		func(c fiber.Ctx, _ httpkit.EmptyPayload) httpkit.Response[[]identity.PermissionView] {
			result, err := h.service.ListPermissions(c.Context())
			if err != nil {
				return httpkit.NewErrorResponse[[]identity.PermissionView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, identity.ViewPermissions(result))
		},
	)
}
