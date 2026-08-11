package workspaces

import (
	"resolved-server/internal/httpkit"
	"resolved-server/internal/problem"

	"github.com/gofiber/fiber/v3"
)

type EnvironmentsRequest struct {
	WorkspaceID string `json:"-" validate:"required"`
}

type EnvironmentsPayload EnvironmentsRequest

type CreateEnvironmentRequest struct {
	WorkspaceID string `json:"-" validate:"required"`
	Name        string `json:"name" validate:"required,max=120"`
}

type CreateEnvironmentPayload CreateEnvironmentRequest

type EnvironmentRequest struct {
	WorkspaceID   string `json:"-" validate:"required"`
	EnvironmentID string `json:"-" validate:"required"`
}

type EnvironmentPayload EnvironmentRequest

type UpdateEnvironmentRequest struct {
	WorkspaceID   string `json:"-" validate:"required"`
	EnvironmentID string `json:"-" validate:"required"`
	Name          string `json:"name" validate:"required,max=120"`
}

type UpdateEnvironmentPayload UpdateEnvironmentRequest

type CreateEnvironmentVariableRequest struct {
	WorkspaceID   string `json:"-" validate:"required"`
	EnvironmentID string `json:"-" validate:"required"`
	Key           string `json:"key" validate:"required,max=256"`
	Value         string `json:"value"`
	Enabled       *bool  `json:"enabled"`
	Secret        bool   `json:"secret"`
}

type CreateEnvironmentVariablePayload struct {
	WorkspaceID   string
	EnvironmentID string
	Key           string
	Value         string
	Enabled       bool
	Secret        bool
}

type EnvironmentVariableRequest struct {
	WorkspaceID   string `json:"-" validate:"required"`
	EnvironmentID string `json:"-" validate:"required"`
	VariableID    string `json:"-" validate:"required"`
}

type EnvironmentVariablePayload EnvironmentVariableRequest

type UpdateEnvironmentVariableRequest struct {
	WorkspaceID   string  `json:"-" validate:"required"`
	EnvironmentID string  `json:"-" validate:"required"`
	VariableID    string  `json:"-" validate:"required"`
	Key           *string `json:"key" validate:"omitempty,max=256"`
	Enabled       *bool   `json:"enabled"`
	Secret        *bool   `json:"secret"`
}

type UpdateEnvironmentVariablePayload UpdateEnvironmentVariableRequest

type PutEnvironmentVariableValueRequest struct {
	WorkspaceID   string  `json:"-" validate:"required"`
	EnvironmentID string  `json:"-" validate:"required"`
	VariableID    string  `json:"-" validate:"required"`
	Value         *string `json:"value"`
}

type PutEnvironmentVariableValuePayload struct {
	WorkspaceID   string
	EnvironmentID string
	VariableID    string
	Value         string
}

func (request *EnvironmentsRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	return nil
}

func (request *EnvironmentsRequest) ToPayload(*httpkit.ProcessingContext) (EnvironmentsPayload, error) {
	return EnvironmentsPayload(*request), nil
}

func (request *EnvironmentsRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *CreateEnvironmentRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	return c.Bind().Body(request)
}

func (request *CreateEnvironmentRequest) ToPayload(*httpkit.ProcessingContext) (CreateEnvironmentPayload, error) {
	return CreateEnvironmentPayload(*request), nil
}

func (request *CreateEnvironmentRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *EnvironmentRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.EnvironmentID = c.Params("environment_id")
	return nil
}

func (request *EnvironmentRequest) ToPayload(*httpkit.ProcessingContext) (EnvironmentPayload, error) {
	return EnvironmentPayload(*request), nil
}

func (request *EnvironmentRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *UpdateEnvironmentRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.EnvironmentID = c.Params("environment_id")
	return c.Bind().Body(request)
}

func (request *UpdateEnvironmentRequest) ToPayload(*httpkit.ProcessingContext) (UpdateEnvironmentPayload, error) {
	return UpdateEnvironmentPayload(*request), nil
}

func (request *UpdateEnvironmentRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *CreateEnvironmentVariableRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.EnvironmentID = c.Params("environment_id")
	return c.Bind().Body(request)
}

func (request *CreateEnvironmentVariableRequest) ToPayload(*httpkit.ProcessingContext) (CreateEnvironmentVariablePayload, error) {
	enabled := true
	if request.Enabled != nil {
		enabled = *request.Enabled
	}
	return CreateEnvironmentVariablePayload{
		WorkspaceID:   request.WorkspaceID,
		EnvironmentID: request.EnvironmentID,
		Key:           request.Key,
		Value:         request.Value,
		Enabled:       enabled,
		Secret:        request.Secret,
	}, nil
}

func (request *CreateEnvironmentVariableRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *EnvironmentVariableRequest) bindRoute(c fiber.Ctx) {
	request.WorkspaceID = c.Params("workspace_id")
	request.EnvironmentID = c.Params("environment_id")
	request.VariableID = c.Params("variable_id")
}

func (request *EnvironmentVariableRequest) BindFiber(c fiber.Ctx) error {
	request.bindRoute(c)
	return nil
}

func (request *EnvironmentVariableRequest) ToPayload(*httpkit.ProcessingContext) (EnvironmentVariablePayload, error) {
	return EnvironmentVariablePayload(*request), nil
}

func (request *EnvironmentVariableRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *UpdateEnvironmentVariableRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.EnvironmentID = c.Params("environment_id")
	request.VariableID = c.Params("variable_id")
	return c.Bind().Body(request)
}

func (request *UpdateEnvironmentVariableRequest) ToPayload(*httpkit.ProcessingContext) (UpdateEnvironmentVariablePayload, error) {
	if request.Key == nil && request.Enabled == nil && request.Secret == nil {
		return UpdateEnvironmentVariablePayload{}, problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"body": "must contain at least one change"},
		)
	}
	return UpdateEnvironmentVariablePayload(*request), nil
}

func (request *UpdateEnvironmentVariableRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *PutEnvironmentVariableValueRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.EnvironmentID = c.Params("environment_id")
	request.VariableID = c.Params("variable_id")
	return c.Bind().Body(request)
}

func (request *PutEnvironmentVariableValueRequest) ToPayload(*httpkit.ProcessingContext) (PutEnvironmentVariableValuePayload, error) {
	if request.Value == nil {
		return PutEnvironmentVariableValuePayload{}, problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"value": "is required"},
		)
	}
	return PutEnvironmentVariableValuePayload{
		WorkspaceID:   request.WorkspaceID,
		EnvironmentID: request.EnvironmentID,
		VariableID:    request.VariableID,
		Value:         *request.Value,
	}, nil
}

func (request *PutEnvironmentVariableValueRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (h *Handler) ListEnvironmentsController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		[]EnvironmentView,
		EnvironmentsPayload,
		EnvironmentsRequest,
	](
		func(c fiber.Ctx, payload EnvironmentsPayload) httpkit.Response[[]EnvironmentView] {
			result, err := h.service.ListEnvironments(c.Context(), actorFromContext(c), payload.WorkspaceID)
			if err != nil {
				return httpkit.NewErrorResponse[[]EnvironmentView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewEnvironments(result))
		},
	)
}

func (h *Handler) CreateEnvironmentController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		EnvironmentView,
		CreateEnvironmentPayload,
		CreateEnvironmentRequest,
	](
		func(c fiber.Ctx, payload CreateEnvironmentPayload) httpkit.Response[EnvironmentView] {
			result, err := h.service.CreateEnvironment(
				c.Context(), actorFromContext(c), payload.WorkspaceID, CreateEnvironmentInput{Name: payload.Name},
			)
			if err != nil {
				return httpkit.NewErrorResponse[EnvironmentView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusCreated, ViewEnvironment(result))
		},
	)
}

func (h *Handler) GetEnvironmentController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		EnvironmentView,
		EnvironmentPayload,
		EnvironmentRequest,
	](
		func(c fiber.Ctx, payload EnvironmentPayload) httpkit.Response[EnvironmentView] {
			result, err := h.service.GetEnvironment(
				c.Context(), actorFromContext(c), payload.WorkspaceID, payload.EnvironmentID,
			)
			if err != nil {
				return httpkit.NewErrorResponse[EnvironmentView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewEnvironment(result))
		},
	)
}

func (h *Handler) UpdateEnvironmentController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		EnvironmentView,
		UpdateEnvironmentPayload,
		UpdateEnvironmentRequest,
	](
		func(c fiber.Ctx, payload UpdateEnvironmentPayload) httpkit.Response[EnvironmentView] {
			result, err := h.service.UpdateEnvironment(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				payload.EnvironmentID,
				UpdateEnvironmentInput{Name: payload.Name},
			)
			if err != nil {
				return httpkit.NewErrorResponse[EnvironmentView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewEnvironment(result))
		},
	)
}

func (h *Handler) DeleteEnvironmentController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		struct{},
		EnvironmentPayload,
		EnvironmentRequest,
	](
		func(c fiber.Ctx, payload EnvironmentPayload) httpkit.Response[struct{}] {
			if err := h.service.DeleteEnvironment(
				c.Context(), actorFromContext(c), payload.WorkspaceID, payload.EnvironmentID,
			); err != nil {
				return httpkit.NewErrorResponse[struct{}](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, struct{}{})
		},
	)
}

func (h *Handler) CreateEnvironmentVariableController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		EnvironmentVariableView,
		CreateEnvironmentVariablePayload,
		CreateEnvironmentVariableRequest,
	](
		func(c fiber.Ctx, payload CreateEnvironmentVariablePayload) httpkit.Response[EnvironmentVariableView] {
			result, err := h.service.CreateEnvironmentVariable(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				payload.EnvironmentID,
				CreateEnvironmentVariableInput{
					Key: payload.Key, Value: payload.Value, Enabled: payload.Enabled, Secret: payload.Secret,
				},
			)
			if err != nil {
				return httpkit.NewErrorResponse[EnvironmentVariableView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusCreated, ViewEnvironmentVariable(result))
		},
	)
}

func (h *Handler) UpdateEnvironmentVariableController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		EnvironmentVariableView,
		UpdateEnvironmentVariablePayload,
		UpdateEnvironmentVariableRequest,
	](
		func(c fiber.Ctx, payload UpdateEnvironmentVariablePayload) httpkit.Response[EnvironmentVariableView] {
			result, err := h.service.UpdateEnvironmentVariable(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				payload.EnvironmentID,
				payload.VariableID,
				UpdateEnvironmentVariableInput{Key: payload.Key, Enabled: payload.Enabled, Secret: payload.Secret},
			)
			if err != nil {
				return httpkit.NewErrorResponse[EnvironmentVariableView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewEnvironmentVariable(result))
		},
	)
}

func (h *Handler) PutEnvironmentVariableValueController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		EnvironmentVariableView,
		PutEnvironmentVariableValuePayload,
		PutEnvironmentVariableValueRequest,
	](
		func(c fiber.Ctx, payload PutEnvironmentVariableValuePayload) httpkit.Response[EnvironmentVariableView] {
			result, err := h.service.PutEnvironmentVariableValue(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				payload.EnvironmentID,
				payload.VariableID,
				payload.Value,
			)
			if err != nil {
				return httpkit.NewErrorResponse[EnvironmentVariableView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewEnvironmentVariable(result))
		},
	)
}

func (h *Handler) DeleteEnvironmentVariableController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		struct{},
		EnvironmentVariablePayload,
		EnvironmentVariableRequest,
	](
		func(c fiber.Ctx, payload EnvironmentVariablePayload) httpkit.Response[struct{}] {
			if err := h.service.DeleteEnvironmentVariable(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				payload.EnvironmentID,
				payload.VariableID,
			); err != nil {
				return httpkit.NewErrorResponse[struct{}](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, struct{}{})
		},
	)
}
