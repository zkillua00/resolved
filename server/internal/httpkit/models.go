package httpkit

import (
	"context"
	"strings"

	"github.com/gofiber/fiber/v3"
)

type Incoming[RequestType any, PayloadType any] interface {
	*RequestType
	BindFiber(fiber.Ctx) error
	ToPayload(*ProcessingContext) (PayloadType, error)
}

type Validatable interface {
	Validate() ValidationErrors
}

type ContextValidatable interface {
	ValidateWithContext(context.Context) (ValidationErrors, error)
}

type Handler[PayloadType any, ReturnType any] func(fiber.Ctx, PayloadType) Response[ReturnType]

type Response[ReturnType any] interface {
	StatusCode() int
	SetRequestID(string)
	internalError() error
}

type ProcessingContext struct {
	RequestCtx      context.Context
	AttachedContext fiber.Ctx
}

type ValidationErrors []error

func (errors ValidationErrors) Error() string {
	messages := make([]string, 0, len(errors))
	for _, err := range errors {
		messages = append(messages, err.Error())
	}
	return strings.Join(messages, ", ")
}
