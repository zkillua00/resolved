package httpkit

import (
	"errors"
	"log/slog"

	"resolved-server/internal/problem"

	"github.com/gofiber/fiber/v3"
	"github.com/gofiber/fiber/v3/middleware/requestid"
)

type ErrorBody struct {
	Code    string            `json:"code"`
	Message string            `json:"message"`
	Fields  map[string]string `json:"fields,omitempty"`
}

type Envelope[ReturnType any] struct {
	RequestID string      `json:"request_id"`
	Success   bool        `json:"success"`
	Data      *ReturnType `json:"data,omitempty"`
	Error     *ErrorBody  `json:"error,omitempty"`
	status    int
	internal  error
}

func (response *Envelope[ReturnType]) StatusCode() int        { return response.status }
func (response *Envelope[ReturnType]) SetRequestID(id string) { response.RequestID = id }
func (response *Envelope[ReturnType]) internalError() error   { return response.internal }

func NewSuccessResponse[ReturnType any](status int, data ReturnType) *Envelope[ReturnType] {
	return &Envelope[ReturnType]{
		Success: true,
		Data:    &data,
		status:  status,
	}
}

func NewErrorResponse[ReturnType any](err error) *Envelope[ReturnType] {
	status, body, internal := responseError(err)
	return &Envelope[ReturnType]{
		Success:  false,
		Error:    &body,
		status:   status,
		internal: internal,
	}
}

func NewValidationErrorResponse[ReturnType any](fields map[string]string) *Envelope[ReturnType] {
	return NewErrorResponse[ReturnType](problem.WithFields(
		"validation_failed",
		"request validation failed",
		fields,
	))
}

func ErrorHandler(c fiber.Ctx, err error) error {
	return sendResponse[any](c, NewErrorResponse[any](err))
}

func sendResponse[ReturnType any](c fiber.Ctx, response Response[ReturnType]) error {
	requestID := requestid.FromContext(c)
	response.SetRequestID(requestID)
	if err := response.internalError(); err != nil {
		slog.Error("request failed", "request_id", requestID, "error", err)
	}
	return c.Status(response.StatusCode()).JSON(response)
}

func responseError(err error) (int, ErrorBody, error) {
	internalBody := ErrorBody{Code: "internal_error", Message: "an internal error occurred"}
	var appError *problem.Error
	if errors.As(err, &appError) {
		if appError.Kind == problem.KindInternal {
			return fiber.StatusInternalServerError, internalBody, err
		}
		return statusForKind(appError.Kind), ErrorBody{
			Code:    appError.Code,
			Message: appError.Message,
			Fields:  appError.Fields,
		}, nil
	}

	var fiberError *fiber.Error
	if errors.As(err, &fiberError) {
		if fiberError.Code >= fiber.StatusInternalServerError {
			return fiberError.Code, internalBody, err
		}
		return fiberError.Code, ErrorBody{
			Code:    codeForStatus(fiberError.Code),
			Message: fiberError.Message,
		}, nil
	}
	return fiber.StatusInternalServerError, internalBody, err
}

func statusForKind(kind problem.Kind) int {
	switch kind {
	case problem.KindBadRequest:
		return fiber.StatusBadRequest
	case problem.KindInvalid:
		return fiber.StatusUnprocessableEntity
	case problem.KindUnauthorized:
		return fiber.StatusUnauthorized
	case problem.KindForbidden:
		return fiber.StatusForbidden
	case problem.KindNotFound:
		return fiber.StatusNotFound
	case problem.KindConflict:
		return fiber.StatusConflict
	case problem.KindRateLimited:
		return fiber.StatusTooManyRequests
	case problem.KindPayloadTooLarge:
		return fiber.StatusRequestEntityTooLarge
	case problem.KindBadGateway:
		return fiber.StatusBadGateway
	case problem.KindGatewayTimeout:
		return fiber.StatusGatewayTimeout
	default:
		return fiber.StatusInternalServerError
	}
}

func codeForStatus(status int) string {
	switch status {
	case fiber.StatusBadRequest:
		return "bad_request"
	case fiber.StatusUnauthorized:
		return "unauthorized"
	case fiber.StatusForbidden:
		return "forbidden"
	case fiber.StatusNotFound:
		return "not_found"
	case fiber.StatusMethodNotAllowed:
		return "method_not_allowed"
	case fiber.StatusTooManyRequests:
		return "rate_limited"
	case fiber.StatusRequestEntityTooLarge:
		return "payload_too_large"
	case fiber.StatusBadGateway:
		return "bad_gateway"
	case fiber.StatusGatewayTimeout:
		return "gateway_timeout"
	default:
		return "request_failed"
	}
}
