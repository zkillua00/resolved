package users

import (
	"resolved-server/internal/httpkit"
	"resolved-server/internal/identity"
	"resolved-server/internal/problem"

	"github.com/gofiber/fiber/v3"
)

type Handler struct {
	service *Service
}

type CreateRequest struct {
	Email       string   `json:"email" validate:"required,email,max=254"`
	DisplayName string   `json:"display_name" validate:"required,max=120"`
	Password    string   `json:"password" validate:"required,min=12,max=128"`
	RoleIDs     []string `json:"role_ids" validate:"dive,required"`
}

type CreatePayload CreateRequest

type GetRequest struct {
	ID string `json:"-" validate:"required"`
}

type GetPayload GetRequest

type UpdateRequest struct {
	ID          string  `json:"-" validate:"required"`
	Email       *string `json:"email" validate:"omitempty,email,max=254"`
	DisplayName *string `json:"display_name" validate:"omitempty,max=120"`
	Password    *string `json:"password" validate:"omitempty,min=12,max=128"`
	Active      *bool   `json:"active"`
}

type UpdatePayload UpdateRequest

type ReplaceRolesRequest struct {
	ID      string   `json:"-" validate:"required"`
	RoleIDs []string `json:"role_ids" validate:"dive,required"`
}

type ReplaceRolesPayload ReplaceRolesRequest

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
	if request.Email == nil && request.DisplayName == nil && request.Password == nil && request.Active == nil {
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

func (request *ReplaceRolesRequest) BindFiber(c fiber.Ctx) error {
	request.ID = c.Params("id")
	return c.Bind().Body(request)
}

func (request *ReplaceRolesRequest) ToPayload(*httpkit.ProcessingContext) (ReplaceRolesPayload, error) {
	return ReplaceRolesPayload(*request), nil
}

func (request *ReplaceRolesRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (h *Handler) ListController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		[]identity.UserView,
		httpkit.EmptyPayload,
		httpkit.EmptyRequest,
	](
		func(c fiber.Ctx, _ httpkit.EmptyPayload) httpkit.Response[[]identity.UserView] {
			result, err := h.service.List(c.Context())
			if err != nil {
				return httpkit.NewErrorResponse[[]identity.UserView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, identity.ViewUsers(result))
		},
	)
}

func (h *Handler) CreateController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		identity.UserView,
		CreatePayload,
		CreateRequest,
	](
		func(c fiber.Ctx, payload CreatePayload) httpkit.Response[identity.UserView] {
			result, err := h.service.Create(c.Context(), CreateInput{
				Email:       payload.Email,
				DisplayName: payload.DisplayName,
				Password:    payload.Password,
				RoleIDs:     payload.RoleIDs,
			})
			if err != nil {
				return httpkit.NewErrorResponse[identity.UserView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusCreated, identity.ViewUser(result))
		},
	)
}

func (h *Handler) GetController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		identity.UserView,
		GetPayload,
		GetRequest,
	](
		func(c fiber.Ctx, payload GetPayload) httpkit.Response[identity.UserView] {
			result, err := h.service.Get(c.Context(), payload.ID)
			if err != nil {
				return httpkit.NewErrorResponse[identity.UserView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, identity.ViewUser(result))
		},
	)
}

func (h *Handler) UpdateController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		identity.UserView,
		UpdatePayload,
		UpdateRequest,
	](
		func(c fiber.Ctx, payload UpdatePayload) httpkit.Response[identity.UserView] {
			result, err := h.service.Update(c.Context(), payload.ID, UpdateInput{
				Email:       payload.Email,
				DisplayName: payload.DisplayName,
				Password:    payload.Password,
				Active:      payload.Active,
			})
			if err != nil {
				return httpkit.NewErrorResponse[identity.UserView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, identity.ViewUser(result))
		},
	)
}

func (h *Handler) ReplaceRolesController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		identity.UserView,
		ReplaceRolesPayload,
		ReplaceRolesRequest,
	](
		func(c fiber.Ctx, payload ReplaceRolesPayload) httpkit.Response[identity.UserView] {
			result, err := h.service.ReplaceRoles(c.Context(), payload.ID, payload.RoleIDs)
			if err != nil {
				return httpkit.NewErrorResponse[identity.UserView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, identity.ViewUser(result))
		},
	)
}
