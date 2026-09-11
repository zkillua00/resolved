package httpkit

import (
	"errors"

	"resolved-server/internal/problem"

	"github.com/gofiber/fiber/v3"
)

func WithProcessedPayload[
	ReturnType any,
	PayloadType any,
	RequestType any,
	RequestPointer Incoming[RequestType, PayloadType],
](handler Handler[PayloadType, ReturnType]) fiber.Handler {
	return func(c fiber.Ctx) error {
		var request RequestPointer = RequestPointer(new(RequestType))
		if err := request.BindFiber(c); err != nil {
			// Explicit policy/validation errors must survive binding. Keep raw
			// parser errors generic so request contents are never echoed back.
			var policyError *problem.Error
			var transportError *fiber.Error
			if errors.As(err, &policyError) ||
				(errors.As(err, &transportError) &&
					(transportError.Code == fiber.StatusRequestEntityTooLarge ||
						transportError.Code == fiber.StatusUnsupportedMediaType)) {
				return sendResponse[ReturnType](c, NewErrorResponse[ReturnType](err))
			}
			return sendResponse[ReturnType](c, NewErrorResponse[ReturnType](
				problem.New(problem.KindBadRequest, "invalid_body", "request body is invalid"),
			))
		}

		if contextValidatable, ok := any(request).(ContextValidatable); ok {
			validationErrors, err := contextValidatable.ValidateWithContext(c.Context())
			if err != nil {
				return sendResponse[ReturnType](c, NewErrorResponse[ReturnType](problem.Wrap(err, "validate request")))
			}
			if len(validationErrors) > 0 {
				return sendResponse[ReturnType](c, NewValidationErrorResponse[ReturnType](ValidationFields(validationErrors)))
			}
		} else if validatable, ok := any(request).(Validatable); ok {
			validationErrors := validatable.Validate()
			if len(validationErrors) > 0 {
				return sendResponse[ReturnType](c, NewValidationErrorResponse[ReturnType](ValidationFields(validationErrors)))
			}
		}

		payload, err := request.ToPayload(&ProcessingContext{
			RequestCtx:      c.Context(),
			AttachedContext: c,
		})
		if err != nil {
			return sendResponse[ReturnType](c, NewErrorResponse[ReturnType](err))
		}
		return sendResponse[ReturnType](c, handler(c, payload))
	}
}
