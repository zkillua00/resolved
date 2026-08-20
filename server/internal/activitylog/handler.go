package activitylog

import (
	"strconv"

	"resolved-server/internal/auth"
	"resolved-server/internal/httpkit"
	"resolved-server/internal/identity"
	"resolved-server/internal/problem"
	"resolved-server/internal/workspaces"

	"github.com/gofiber/fiber/v3"
)

type Handler struct {
	service *Service
}

type WorkspaceLogRequest struct {
	WorkspaceID string `json:"-" validate:"required"`
	Cursor      string `json:"-"`
	After       string `json:"-"`
	Limit       int    `json:"-"`
}

type WorkspaceLogPayload WorkspaceLogRequest

type AuditLogRequest struct {
	Cursor string `json:"-"`
	After  string `json:"-"`
	Limit  int    `json:"-"`
}

type AuditLogPayload AuditLogRequest

func NewHandler(service *Service) *Handler {
	return &Handler{service: service}
}

func (request *WorkspaceLogRequest) BindFiber(c fiber.Ctx) error {
	request.WorkspaceID = c.Params("workspace_id")
	return bindPageQuery(c, &request.Cursor, &request.After, &request.Limit)
}

func (request *WorkspaceLogRequest) ToPayload(*httpkit.ProcessingContext) (WorkspaceLogPayload, error) {
	return WorkspaceLogPayload(*request), nil
}

func (request *WorkspaceLogRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (request *AuditLogRequest) BindFiber(c fiber.Ctx) error {
	return bindPageQuery(c, &request.Cursor, &request.After, &request.Limit)
}

func (request *AuditLogRequest) ToPayload(*httpkit.ProcessingContext) (AuditLogPayload, error) {
	return AuditLogPayload(*request), nil
}

func (request *AuditLogRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (h *Handler) WorkspaceController() fiber.Handler {
	return httpkit.WithProcessedPayload[PageView, WorkspaceLogPayload, WorkspaceLogRequest](
		func(c fiber.Ctx, payload WorkspaceLogPayload) httpkit.Response[PageView] {
			page, err := h.service.ListWorkspace(
				c.Context(),
				actorFromContext(c),
				payload.WorkspaceID,
				ListInput{Cursor: payload.Cursor, After: payload.After, Limit: payload.Limit},
			)
			if err != nil {
				return httpkit.NewErrorResponse[PageView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, page)
		},
	)
}

func (h *Handler) AuditController() fiber.Handler {
	return httpkit.WithProcessedPayload[PageView, AuditLogPayload, AuditLogRequest](
		func(c fiber.Ctx, payload AuditLogPayload) httpkit.Response[PageView] {
			page, err := h.service.ListAudit(
				c.Context(),
				ListInput{Cursor: payload.Cursor, After: payload.After, Limit: payload.Limit},
			)
			if err != nil {
				return httpkit.NewErrorResponse[PageView](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, page)
		},
	)
}

func bindPageQuery(c fiber.Ctx, cursor, after *string, limit *int) error {
	*cursor = c.Query("cursor")
	*after = c.Query("after")
	value := c.Query("limit")
	if value == "" {
		*limit = DefaultPageLimit
		return nil
	}
	parsed, err := strconv.Atoi(value)
	if err != nil {
		return problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"limit": "must be a number between 1 and 100"},
		)
	}
	*limit = parsed
	return nil
}

func actorFromContext(c fiber.Ctx) workspaces.Actor {
	principal := auth.PrincipalFromContext(c)
	return workspaces.Actor{
		UserID: principal.User.ID,
		Owner:  principal.HasRole(identity.OwnerRoleID),
	}
}
