package executionlimits

import (
	"bytes"
	"database/sql"
	"encoding/json"
	"io"

	"resolved-server/internal/auth"
	"resolved-server/internal/httpkit"
	"resolved-server/internal/identity"
	"resolved-server/internal/problem"
	"resolved-server/internal/resourceevents"
	"resolved-server/internal/workspaces"

	"github.com/gofiber/fiber/v3"
	"github.com/gofiber/fiber/v3/middleware/requestid"
	"gorm.io/gorm"
)

type Handler struct {
	provider *Provider
	events   resourceevents.Emitter
}

func NewHandler(provider *Provider, events resourceevents.Emitter) *Handler {
	return &Handler{provider: provider, events: events}
}

// authorize uses the same direct-workspace/inherited-collection grant rule as
// workspaces.Service, but inside the policy snapshot transaction. A descendant
// grant never authorizes changing its ancestor or the entire workspace.
func authorize(tx *gorm.DB, layers []Scope, userID string, owner bool) error {
	if owner || len(layers) == 1 {
		return nil
	}
	var count int64
	if err := tx.Model(&workspaces.WorkspaceUser{}).Where("workspace_id = ? AND user_id = ?", layers[1].WorkspaceID, userID).Count(&count).Error; err != nil {
		return err
	}
	if count > 0 {
		return nil
	}
	for _, layer := range layers[2:] {
		if err := tx.Model(&workspaces.CollectionUser{}).Where("collection_id = ? AND user_id = ?", layer.CollectionID, userID).Count(&count).Error; err != nil {
			return err
		}
		if count > 0 {
			return nil
		}
	}
	return problem.New(problem.KindForbidden, "scope_access_denied", "the account does not have access to this execution limit scope")
}

func (h *Handler) Controller() fiber.Handler {
	return func(c fiber.Ctx) error {
		scope := Scope{WorkspaceID: c.Query("workspace_id"), CollectionID: c.Query("collection_id")}
		principal := auth.PrincipalFromContext(c)
		permission := "server_settings"
		if scope.WorkspaceID != "" {
			permission = "workspaces"
		}
		if scope.CollectionID != "" {
			permission = "collections"
		}
		write := c.Method() == fiber.MethodPut
		if write {
			permission += ".update"
		} else {
			permission += ".read"
		}
		if principal == nil || !principal.HasPermission(permission) {
			return problem.New(problem.KindForbidden, "forbidden", "the account does not have the required permission")
		}
		check := func(tx *gorm.DB, layers []Scope) error {
			return authorize(tx, layers, principal.User.ID, principal.HasRole(identity.OwnerRoleID))
		}
		var snapshot Snapshot
		var err error
		if write {
			var payload struct {
				Overrides *Limits `json:"overrides"`
			}
			decoder := json.NewDecoder(bytes.NewReader(c.Body()))
			decoder.DisallowUnknownFields()
			if decoder.Decode(&payload) != nil || payload.Overrides == nil || decoder.Decode(new(any)) != io.EOF {
				return problem.New(problem.KindBadRequest, "invalid_body", "body must contain an overrides object")
			}
			var before Limits
			snapshot, before, err = h.provider.replace(c.Context(), scope, *payload.Overrides, check)
			if err == nil {
				resourceevents.Emit(h.events, resourceevents.Change{
					Resource: resourceevents.ResourceServerSettings, Action: resourceevents.ActionUpdated,
					ResourceID: "request-execution-limits", WorkspaceID: scope.WorkspaceID, CollectionID: scope.CollectionID,
					ActorUserID: principal.User.ID, TargetName: "Request execution limits",
					Audience: resourceevents.Audience{Owners: true},
					Diffs:    []resourceevents.Diff{{Field: "execution_limits", From: before, To: snapshot.Overrides}},
				})
				// All authenticated clients may need to reload inherited policy.
				// This invalidation deliberately exposes neither scope IDs nor values.
				resourceevents.Emit(h.events, resourceevents.Change{
					Resource: "request_execution_limits", Action: resourceevents.ActionUpdated,
					ResourceID: "request-execution-limits", Audience: resourceevents.Audience{Everyone: true},
				})
			}
		} else {
			err = h.provider.db.WithContext(c.Context()).Transaction(func(tx *gorm.DB) error {
				layers, err := ancestry(tx, scope)
				if err != nil {
					return err
				}
				if err := check(tx, layers); err != nil {
					return err
				}
				snapshot, err = resolve(tx, layers)
				return err
			}, &sql.TxOptions{Isolation: sql.LevelRepeatableRead, ReadOnly: true})
		}
		if err != nil {
			return err
		}
		response := httpkit.NewSuccessResponse(fiber.StatusOK, snapshot)
		response.SetRequestID(requestid.FromContext(c))
		return c.Status(fiber.StatusOK).JSON(response)
	}
}
