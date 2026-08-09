package auth

import (
	"time"

	"resolved-server/internal/httpkit"
	"resolved-server/internal/identity"

	"github.com/gofiber/fiber/v3"
)

type Handler struct {
	service *Service
}

type LoginRequest struct {
	Email    string `json:"email" validate:"required,email,max=254"`
	Password string `json:"password" validate:"required,max=128"`
}

type LoginPayload LoginRequest

type LoginResponse struct {
	Token     string            `json:"token"`
	TokenType string            `json:"token_type"`
	ExpiresAt time.Time         `json:"expires_at"`
	User      identity.UserView `json:"user"`
}

func NewHandler(service *Service) *Handler {
	return &Handler{service: service}
}

func (request *LoginRequest) BindFiber(c fiber.Ctx) error {
	return c.Bind().Body(request)
}

func (request *LoginRequest) ToPayload(*httpkit.ProcessingContext) (LoginPayload, error) {
	return LoginPayload(*request), nil
}

func (request *LoginRequest) Validate() httpkit.ValidationErrors {
	return httpkit.DefaultValidation(request)
}

func (h *Handler) LoginController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		LoginResponse,
		LoginPayload,
		LoginRequest,
	](
		func(c fiber.Ctx, payload LoginPayload) httpkit.Response[LoginResponse] {
			result, err := h.service.Login(c.Context(), payload.Email, payload.Password)
			if err != nil {
				return httpkit.NewErrorResponse[LoginResponse](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, LoginResponse{
				Token:     result.Token,
				TokenType: "Bearer",
				ExpiresAt: result.ExpiresAt,
				User:      identity.ViewUser(result.User),
			})
		},
	)
}

func (h *Handler) LogoutController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		struct{},
		httpkit.EmptyPayload,
		httpkit.EmptyRequest,
	](
		func(c fiber.Ctx, _ httpkit.EmptyPayload) httpkit.Response[struct{}] {
			if err := h.service.Logout(c.Context(), TokenFromContext(c)); err != nil {
				return httpkit.NewErrorResponse[struct{}](err)
			}
			return httpkit.NewSuccessResponse(fiber.StatusOK, struct{}{})
		},
	)
}

func (h *Handler) MeController() fiber.Handler {
	return httpkit.WithProcessedPayload[
		identity.UserView,
		httpkit.EmptyPayload,
		httpkit.EmptyRequest,
	](
		func(c fiber.Ctx, _ httpkit.EmptyPayload) httpkit.Response[identity.UserView] {
			principal := PrincipalFromContext(c)
			return httpkit.NewSuccessResponse(fiber.StatusOK, identity.ViewUser(principal.User))
		},
	)
}
