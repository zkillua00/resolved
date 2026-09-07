package workspaces

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"

	"resolved-server/internal/auth"
	"resolved-server/internal/httpkit"
	"resolved-server/internal/identity"
	"resolved-server/internal/problem"

	"github.com/gofiber/fiber/v3"
)

type Handler struct {
	service *Service
}

type CreateWorkspaceRequest struct {
	Name string `json:"name" validate:"required,max=120"`
}

type CreateWorkspacePayload CreateWorkspaceRequest

type WorkspaceRequest struct {
	WorkspaceID string `json:"-" validate:"required"`
}

type WorkspacePayload WorkspaceRequest

type UpdateWorkspaceRequest struct {
	WorkspaceID string `json:"-" validate:"required"`
	Name        string `json:"name" validate:"required,max=120"`
}

type UpdateWorkspacePayload UpdateWorkspaceRequest

type ReplaceWorkspaceUsersRequest struct {
	WorkspaceID string   `json:"-" validate:"required"`
	UserIDs     []string `json:"user_ids" validate:"dive,required"`
}

type ReplaceWorkspaceUsersPayload ReplaceWorkspaceUsersRequest

type CreateCollectionRequest struct {
	WorkspaceID        string  `json:"-" validate:"required"`
	Name               string  `json:"name" validate:"required,max=120"`
	ParentCollectionID *string `json:"parent_collection_id"`
}

type CreateCollectionPayload CreateCollectionRequest

type CollectionRequest struct {
	WorkspaceID  string `json:"-" validate:"required"`
	CollectionID string `json:"-" validate:"required"`
}

type CollectionPayload CollectionRequest

type UpdateCollectionRequest struct {
	WorkspaceID  string `json:"-" validate:"required"`
	CollectionID string `json:"-" validate:"required"`
	Name         string `json:"name" validate:"required,max=120"`
}

type UpdateCollectionPayload UpdateCollectionRequest

type nullableString struct {
	Value *string
	Set   bool
}

type MoveCollectionRequest struct {
	WorkspaceID        string         `json:"-" validate:"required"`
	CollectionID       string         `json:"-" validate:"required"`
	ParentCollectionID nullableString `json:"parent_collection_id"`
}

type MoveCollectionPayload struct {
	WorkspaceID        string
	CollectionID       string
	ParentCollectionID *string
}

type ReplaceCollectionUsersRequest struct {
	WorkspaceID  string   `json:"-" validate:"required"`
	CollectionID string   `json:"-" validate:"required"`
	UserIDs      []string `json:"user_ids" validate:"dive,required"`
}

type ReplaceCollectionUsersPayload ReplaceCollectionUsersRequest

type CreateSavedRequestRequest struct {
	WorkspaceID  string          `json:"-" validate:"required"`
	CollectionID string          `json:"-" validate:"required"`
	Name         string          `json:"name" validate:"required,max=120"`
	Definition   json.RawMessage `json:"definition" validate:"required"`
}

type CreateSavedRequestPayload CreateSavedRequestRequest

type SavedRequestRequest struct {
	WorkspaceID  string `json:"-" validate:"required"`
	CollectionID string `json:"-" validate:"required"`
	RequestID    string `json:"-" validate:"required"`
}

type SavedRequestPayload SavedRequestRequest

type UpdateSavedRequestRequest struct {
	WorkspaceID  string          `json:"-" validate:"required"`
	CollectionID string          `json:"-" validate:"required"`
	RequestID    string          `json:"-" validate:"required"`
	Name         string          `json:"name" validate:"required,max=120"`
	Definition   json.RawMessage `json:"definition" validate:"required"`
}

type UpdateSavedRequestPayload UpdateSavedRequestRequest

type MoveSavedRequestRequest struct {
	WorkspaceID        string `json:"-" validate:"required"`
	CollectionID       string `json:"-" validate:"required"`
	RequestID          string `json:"-" validate:"required"`
	TargetCollectionID string `json:"target_collection_id" validate:"required"`
}

type MoveSavedRequestPayload MoveSavedRequestRequest

func NewHandler(service *Service) *Handler {
	return &Handler{service: service}
}

func (request *CreateWorkspaceRequest) BindFiber(c fiber.Ctx) error {
	return c.Bind().Body(request)
}

func (request *CreateWorkspaceRequest) ToPayload(*httpkit.ProcessingContext) (CreateWorkspacePayload, error) {
	return CreateWorkspacePayload(*request), nil
}

func (request *CreateWorkspaceRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *WorkspaceRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	return nil
}

func (request *WorkspaceRequest) ToPayload(*httpkit.ProcessingContext) (WorkspacePayload, error) {
	return WorkspacePayload(*request), nil
}

func (request *WorkspaceRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *UpdateWorkspaceRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	return c.Bind().Body(request)
}

func (request *UpdateWorkspaceRequest) ToPayload(*httpkit.ProcessingContext) (UpdateWorkspacePayload, error) {
	return UpdateWorkspacePayload(*request), nil
}

func (request *UpdateWorkspaceRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *ReplaceWorkspaceUsersRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	return c.Bind().Body(request)
}

func (request *ReplaceWorkspaceUsersRequest) ToPayload(*httpkit.ProcessingContext) (ReplaceWorkspaceUsersPayload, error) {
	return ReplaceWorkspaceUsersPayload(*request), nil
}

func (request *ReplaceWorkspaceUsersRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *CreateCollectionRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	return c.Bind().Body(request)
}

func (request *CreateCollectionRequest) ToPayload(*httpkit.ProcessingContext) (CreateCollectionPayload, error) {
	return CreateCollectionPayload(*request), nil
}

func (request *CreateCollectionRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *CollectionRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.CollectionID = c.Params("collection_id")
	return nil
}

func (request *CollectionRequest) ToPayload(*httpkit.ProcessingContext) (CollectionPayload, error) {
	return CollectionPayload(*request), nil
}

func (request *CollectionRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *UpdateCollectionRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.CollectionID = c.Params("collection_id")
	return c.Bind().Body(request)
}

func (request *UpdateCollectionRequest) ToPayload(*httpkit.ProcessingContext) (UpdateCollectionPayload, error) {
	return UpdateCollectionPayload(*request), nil
}

func (request *UpdateCollectionRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (value *nullableString) UnmarshalJSON(data []byte) error {
	value.Set = true
	if bytes.Equal(bytes.TrimSpace(data), []byte("null")) {
		value.Value = nil
		return nil
	}
	var decoded string
	if err := json.Unmarshal(data, &decoded); err != nil {
		return err
	}
	value.Value = &decoded
	return nil
}

func (request *MoveCollectionRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.CollectionID = c.Params("collection_id")
	return c.Bind().Body(request)
}

func (request *MoveCollectionRequest) ToPayload(*httpkit.ProcessingContext) (MoveCollectionPayload, error) {
	if !request.ParentCollectionID.Set {
		return MoveCollectionPayload{}, problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"parent_collection_id": "is required"},
		)
	}
	return MoveCollectionPayload{
		WorkspaceID:        request.WorkspaceID,
		CollectionID:       request.CollectionID,
		ParentCollectionID: request.ParentCollectionID.Value,
	}, nil
}

func (request *MoveCollectionRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *ReplaceCollectionUsersRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.CollectionID = c.Params("collection_id")
	return c.Bind().Body(request)
}

func (request *ReplaceCollectionUsersRequest) ToPayload(*httpkit.ProcessingContext) (ReplaceCollectionUsersPayload, error) {
	return ReplaceCollectionUsersPayload(*request), nil
}

func (request *ReplaceCollectionUsersRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *CreateSavedRequestRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.CollectionID = c.Params("collection_id")
	return c.Bind().Body(request)
}

func (request *CreateSavedRequestRequest) ToPayload(*httpkit.ProcessingContext) (CreateSavedRequestPayload, error) {
	return CreateSavedRequestPayload(*request), nil
}

func (request *CreateSavedRequestRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *SavedRequestRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.CollectionID = c.Params("collection_id")
	request.RequestID = c.Params("request_id")
	return nil
}

func (request *SavedRequestRequest) ToPayload(*httpkit.ProcessingContext) (SavedRequestPayload, error) {
	return SavedRequestPayload(*request), nil
}

func (request *SavedRequestRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *UpdateSavedRequestRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.CollectionID = c.Params("collection_id")
	request.RequestID = c.Params("request_id")
	return c.Bind().Body(request)
}

func (request *UpdateSavedRequestRequest) ToPayload(*httpkit.ProcessingContext) (UpdateSavedRequestPayload, error) {
	return UpdateSavedRequestPayload(*request), nil
}

func (request *UpdateSavedRequestRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *MoveSavedRequestRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	request.CollectionID = c.Params("collection_id")
	request.RequestID = c.Params("request_id")
	return c.Bind().Body(request)
}

func (request *MoveSavedRequestRequest) ToPayload(*httpkit.ProcessingContext) (MoveSavedRequestPayload, error) {
	return MoveSavedRequestPayload(*request), nil
}

func (request *MoveSavedRequestRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (h *Handler) ListController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		[]WorkspaceView,
		httpkit.EmptyPayload,
		httpkit.EmptyRequest,
	](
		func(c fiber.Ctx, _ httpkit.EmptyPayload) httpkit.Response[[]WorkspaceView] {
			result, err := h.service.List(c.Context(), actorFromContext(c))
			if err != nil {
				return httpkit.NewErrorResponse[[]WorkspaceView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewWorkspaces(result))
		},
	)
}

func (h *Handler) CreateController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		WorkspaceView,
		CreateWorkspacePayload,
		CreateWorkspaceRequest,
	](
		func(c fiber.Ctx, payload CreateWorkspacePayload) httpkit.Response[WorkspaceView] {
			result, err := h.service.Create(c.Context(), actorFromContext(c), CreateWorkspaceInput{Name: payload.Name})
			if err != nil {
				return httpkit.NewErrorResponse[WorkspaceView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusCreated, ViewWorkspace(result))
		},
	)
}

func (h *Handler) GetController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		WorkspaceView,
		WorkspacePayload,
		WorkspaceRequest,
	](
		func(c fiber.Ctx, payload WorkspacePayload) httpkit.Response[WorkspaceView] {
			result, err := h.service.Get(c.Context(), actorFromContext(c), payload.WorkspaceID)
			if err != nil {
				return httpkit.NewErrorResponse[WorkspaceView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewWorkspace(result))
		},
	)
}

func (h *Handler) UpdateController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		WorkspaceView,
		UpdateWorkspacePayload,
		UpdateWorkspaceRequest,
	](
		func(c fiber.Ctx, payload UpdateWorkspacePayload) httpkit.Response[WorkspaceView] {
			result, err := h.service.Update(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				UpdateWorkspaceInput{Name: payload.Name},
			)
			if err != nil {
				return httpkit.NewErrorResponse[WorkspaceView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewWorkspace(result))
		},
	)
}

func (h *Handler) ReplaceUsersController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		WorkspaceView,
		ReplaceWorkspaceUsersPayload,
		ReplaceWorkspaceUsersRequest,
	](
		func(c fiber.Ctx, payload ReplaceWorkspaceUsersPayload) httpkit.Response[WorkspaceView] {
			result, err := h.service.ReplaceWorkspaceUsers(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				payload.UserIDs,
			)
			if err != nil {
				return httpkit.NewErrorResponse[WorkspaceView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewWorkspace(result))
		},
	)
}

func (h *Handler) DeleteController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		struct{},
		WorkspacePayload,
		WorkspaceRequest,
	](
		func(c fiber.Ctx, payload WorkspacePayload) httpkit.Response[struct{}] {
			if err := h.service.Delete(c.Context(), actorFromContext(c), payload.WorkspaceID); err != nil {
				return httpkit.NewErrorResponse[struct{}](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, struct{}{})
		},
	)
}

func (h *Handler) CreateCollectionController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		CollectionView,
		CreateCollectionPayload,
		CreateCollectionRequest,
	](
		func(c fiber.Ctx, payload CreateCollectionPayload) httpkit.Response[CollectionView] {
			result, err := h.service.CreateCollection(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				CreateCollectionInput{
					Name:               payload.Name,
					ParentCollectionID: payload.ParentCollectionID,
				},
			)
			if err != nil {
				return httpkit.NewErrorResponse[CollectionView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusCreated, ViewCollection(result))
		},
	)
}

func (h *Handler) GetCollectionController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		CollectionView,
		CollectionPayload,
		CollectionRequest,
	](
		func(c fiber.Ctx, payload CollectionPayload) httpkit.Response[CollectionView] {
			result, err := h.service.GetCollection(
				c.Context(), actorFromContext(c), payload.WorkspaceID, payload.CollectionID,
			)
			if err != nil {
				return httpkit.NewErrorResponse[CollectionView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewCollection(result))
		},
	)
}

func (h *Handler) UpdateCollectionController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		CollectionView,
		UpdateCollectionPayload,
		UpdateCollectionRequest,
	](
		func(c fiber.Ctx, payload UpdateCollectionPayload) httpkit.Response[CollectionView] {
			result, err := h.service.UpdateCollection(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				payload.CollectionID,
				UpdateCollectionInput{Name: payload.Name},
			)
			if err != nil {
				return httpkit.NewErrorResponse[CollectionView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewCollection(result))
		},
	)
}

func (h *Handler) MoveCollectionController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		CollectionView,
		MoveCollectionPayload,
		MoveCollectionRequest,
	](
		func(c fiber.Ctx, payload MoveCollectionPayload) httpkit.Response[CollectionView] {
			result, err := h.service.MoveCollection(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				payload.CollectionID,
				payload.ParentCollectionID,
			)
			if err != nil {
				return httpkit.NewErrorResponse[CollectionView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewCollection(result))
		},
	)
}

func (h *Handler) ReplaceCollectionUsersController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		CollectionView,
		ReplaceCollectionUsersPayload,
		ReplaceCollectionUsersRequest,
	](
		func(c fiber.Ctx, payload ReplaceCollectionUsersPayload) httpkit.Response[CollectionView] {
			result, err := h.service.ReplaceCollectionUsers(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				payload.CollectionID,
				payload.UserIDs,
			)
			if err != nil {
				return httpkit.NewErrorResponse[CollectionView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewCollection(result))
		},
	)
}

func (h *Handler) DeleteCollectionController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		struct{},
		CollectionPayload,
		CollectionRequest,
	](
		func(c fiber.Ctx, payload CollectionPayload) httpkit.Response[struct{}] {
			if err := h.service.DeleteCollection(
				c.Context(), actorFromContext(c), payload.WorkspaceID, payload.CollectionID,
			); err != nil {
				return httpkit.NewErrorResponse[struct{}](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, struct{}{})
		},
	)
}

func (h *Handler) CreateSavedRequestController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		SavedRequestView,
		CreateSavedRequestPayload,
		CreateSavedRequestRequest,
	](
		func(c fiber.Ctx, payload CreateSavedRequestPayload) httpkit.Response[SavedRequestView] {
			result, err := h.service.CreateSavedRequest(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				payload.CollectionID,
				CreateSavedRequestInput{Name: payload.Name, Definition: payload.Definition},
			)
			if err != nil {
				return httpkit.NewErrorResponse[SavedRequestView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusCreated, ViewSavedRequest(result))
		},
	)
}

func (h *Handler) GetSavedRequestController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		SavedRequestView,
		SavedRequestPayload,
		SavedRequestRequest,
	](
		func(c fiber.Ctx, payload SavedRequestPayload) httpkit.Response[SavedRequestView] {
			result, err := h.service.GetSavedRequest(
				c.Context(), actorFromContext(c), payload.WorkspaceID, payload.CollectionID, payload.RequestID,
			)
			if err != nil {
				return httpkit.NewErrorResponse[SavedRequestView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewSavedRequest(result))
		},
	)
}

func (h *Handler) UpdateSavedRequestController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		SavedRequestView,
		UpdateSavedRequestPayload,
		UpdateSavedRequestRequest,
	](
		func(c fiber.Ctx, payload UpdateSavedRequestPayload) httpkit.Response[SavedRequestView] {
			result, err := h.service.UpdateSavedRequest(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				payload.CollectionID,
				payload.RequestID,
				UpdateSavedRequestInput{Name: payload.Name, Definition: payload.Definition},
			)
			if err != nil {
				return httpkit.NewErrorResponse[SavedRequestView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewSavedRequest(result))
		},
	)
}

func (h *Handler) MoveSavedRequestController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		SavedRequestView,
		MoveSavedRequestPayload,
		MoveSavedRequestRequest,
	](
		func(c fiber.Ctx, payload MoveSavedRequestPayload) httpkit.Response[SavedRequestView] {
			result, err := h.service.MoveSavedRequest(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				payload.CollectionID,
				payload.RequestID,
				payload.TargetCollectionID,
			)
			if err != nil {
				return httpkit.NewErrorResponse[SavedRequestView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, ViewSavedRequest(result))
		},
	)
}

func (h *Handler) DeleteSavedRequestController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		struct{},
		SavedRequestPayload,
		SavedRequestRequest,
	](
		func(c fiber.Ctx, payload SavedRequestPayload) httpkit.Response[struct{}] {
			if err := h.service.DeleteSavedRequest(
				c.Context(), actorFromContext(c), payload.WorkspaceID, payload.CollectionID, payload.RequestID,
			); err != nil {
				return httpkit.NewErrorResponse[struct{}](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, struct{}{})
		},
	)
}

func actorFromContext(c fiber.Ctx) Actor {
	principal := auth.PrincipalFromContext(c)
	return Actor{
		UserID:            principal.User.ID,
		Owner:             principal.HasRole(identity.OwnerRoleID),
		EnvironmentKey:    principal.EnvironmentKey(),
		CredentialVersion: sha256.Sum256([]byte(principal.User.PasswordHash)),
	}
}
