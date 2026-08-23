package sharedhistory

import (
	"time"

	"resolved-server/internal/auth"
	"resolved-server/internal/httpkit"
	"resolved-server/internal/identity"
	"resolved-server/internal/workspaces"

	"github.com/gofiber/fiber/v3"
)

type Handler struct {
	service *Service
}

type ListRequest struct {
	UserID      string `json:"-" validate:"required"`
	WorkspaceID string `json:"-" validate:"required"`
}

type ListPayload ListRequest

type CreateRequest struct {
	WorkspaceID   string        `json:"-" validate:"required"`
	ClientEntryID string        `json:"client_entry_id" validate:"required,max=128"`
	CreatedAt     time.Time     `json:"created_at" validate:"required"`
	Request       RequestView   `json:"request"`
	Response      *ResponseView `json:"response"`
	Error         string        `json:"error" validate:"max=65536"`
}

type CreatePayload CreateRequest

type DeleteRequest struct {
	WorkspaceID string `json:"-" validate:"required"`
}

type DeletePayload DeleteRequest

func NewHandler(service *Service) *Handler {
	return &Handler{service: service}
}

func (request *ListRequest) BindFiber(c fiber.Ctx) error {
	request.UserID = c.Params("user_id")
	request.WorkspaceID = c.Query("workspace_id")
	return nil
}

func (request *ListRequest) ToPayload(*httpkit.ProcessingContext) (ListPayload, error) {
	return ListPayload(*request), nil
}

func (request *ListRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *CreateRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	return c.Bind().Body(request)
}

func (request *CreateRequest) ToPayload(*httpkit.ProcessingContext) (CreatePayload, error) {
	return CreatePayload(*request), nil
}

func (request *CreateRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *DeleteRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	return nil
}

func (request *DeleteRequest) ToPayload(*httpkit.ProcessingContext) (DeletePayload, error) {
	return DeletePayload(*request), nil
}

func (request *DeleteRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (h *Handler) ListProfilesController() fiber.Handler {
	return httpkit.WithProcessedPayload[[]ProfileView, httpkit.EmptyPayload, httpkit.EmptyRequest](
		func(c fiber.Ctx, _ httpkit.EmptyPayload) httpkit.Response[[]ProfileView] {
			principal := auth.PrincipalFromContext(c)
			profiles, err := h.service.ListProfiles(
				c.Context(),
				actorFromContext(c),
				principal.HasPermission(identity.PermissionUsersRead) ||
					principal.HasPermission(identity.PermissionHistoryReadOthers),
			)
			if err != nil {
				return httpkit.NewErrorResponse[[]ProfileView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, profiles)
		},
	)
}

func (h *Handler) ListController() fiber.Handler {
	return httpkit.WithProcessedPayload[[]EntryView, ListPayload, ListRequest](
		func(c fiber.Ctx, payload ListPayload) httpkit.Response[[]EntryView] {
			principal := auth.PrincipalFromContext(c)
			entries, err := h.service.List(
				c.Context(), actorFromContext(c),
				principal.HasPermission(identity.PermissionHistoryReadOthers),
				payload.WorkspaceID, payload.UserID,
			)
			if err != nil {
				return httpkit.NewErrorResponse[[]EntryView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, entries)
		},
	)
}

func (h *Handler) CreateController() fiber.Handler {
	return httpkit.WithProcessedPayload[EntryView, CreatePayload, CreateRequest](
		func(c fiber.Ctx, payload CreatePayload) httpkit.Response[EntryView] {
			entry, err := h.service.Create(c.Context(), actorFromContext(c), payload.WorkspaceID, CreateInput{
				ClientEntryID: payload.ClientEntryID, CreatedAt: payload.CreatedAt,
				Request: payload.Request, Response: payload.Response, Error: payload.Error,
			})
			if err != nil {
				return httpkit.NewErrorResponse[EntryView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusCreated, entry)
		},
	)
}

func (h *Handler) DeleteController() fiber.Handler {
	return httpkit.WithProcessedPayload[map[string]bool, DeletePayload, DeleteRequest](
		func(c fiber.Ctx, payload DeletePayload) httpkit.Response[map[string]bool] {
			if err := h.service.DeleteOwn(c.Context(), actorFromContext(c), payload.WorkspaceID); err != nil {
				return httpkit.NewErrorResponse[map[string]bool](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, map[string]bool{"deleted": true})
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
