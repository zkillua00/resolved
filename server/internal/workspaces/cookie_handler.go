package workspaces

import (
	"github.com/gofiber/fiber/v3"
	"resolved-server/internal/httpkit"
)

type CookieJarRequest struct {
	WorkspaceID string      `json:"-" validate:"required"`
	Enabled     bool        `json:"enabled"`
	Cookies     []JarCookie `json:"cookies" validate:"max=512"`
	Revision    uint64      `json:"revision"`
	Reset       bool        `json:"reset"`
}
type CookieJarPayload CookieJarRequest

func (r *CookieJarRequest) BindFiber(c fiber.Ctx) error {
	r.WorkspaceID = c.Params("workspace_id")
	if c.Method() == fiber.MethodGet {
		return nil
	}
	return c.Bind().Body(r)
}
func (r *CookieJarRequest) ToPayload(*httpkit.ProcessingContext) (CookieJarPayload, error) {
	return CookieJarPayload(*r), nil
}
func (r *CookieJarRequest) Validate() httpkit.ValidationErrors { return httpkit.DefaultValidation(r) }
func (h *Handler) CookieJarController() fiber.Handler {
	return httpkit.WithProcessedPayload[CookieJar, CookieJarPayload, CookieJarRequest](func(c fiber.Ctx, p CookieJarPayload) httpkit.Response[CookieJar] {
		var jar CookieJar
		var err error
		if c.Method() == fiber.MethodGet {
			jar, err = h.service.GetCookieJar(c.Context(), actorFromContext(c), p.WorkspaceID)
		} else {
			jar, err = h.service.PutCookieJar(c.Context(), actorFromContext(c), p.WorkspaceID, CookieJar{Enabled: p.Enabled, Cookies: p.Cookies, Revision: p.Revision}, p.Reset)
		}
		if err != nil {
			return httpkit.NewErrorResponse[CookieJar](err)
		}
		return httpkit.NewSuccessResponse(fiber.StatusOK, jar)
	})
}
