package httpkit

import (
	"errors"
	"fmt"
	"reflect"
	"strings"

	"github.com/go-playground/validator/v10"
	"github.com/gofiber/fiber/v3"
)

var requestValidator = newValidator()

type EmptyRequest struct{}
type EmptyPayload = EmptyRequest

func (*EmptyRequest) BindFiber(fiber.Ctx) error { return nil }

func (request *EmptyRequest) ToPayload(*ProcessingContext) (EmptyPayload, error) {
	return *request, nil
}

func DefaultValidation(request any) ValidationErrors {
	err := requestValidator.Struct(request)
	if err == nil {
		return nil
	}
	var validationErrors validator.ValidationErrors
	if !errors.As(err, &validationErrors) {
		return ValidationErrors{err}
	}
	result := make(ValidationErrors, 0, len(validationErrors))
	for _, validationError := range validationErrors {
		result = append(result, validationError)
	}
	return result
}

func ValidationFields(validationErrors ValidationErrors) map[string]string {
	fields := make(map[string]string, len(validationErrors))
	for _, err := range validationErrors {
		fieldError, ok := err.(validator.FieldError)
		if !ok {
			fields["body"] = "is invalid"
			continue
		}
		field := fieldError.Field()
		if _, exists := fields[field]; exists {
			continue
		}
		fields[field] = validationMessage(fieldError)
	}
	return fields
}

func newValidator() *validator.Validate {
	validate := validator.New()
	validate.RegisterTagNameFunc(func(field reflect.StructField) string {
		name := strings.SplitN(field.Tag.Get("json"), ",", 2)[0]
		if name == "" || name == "-" {
			return field.Name
		}
		return name
	})
	return validate
}

func validationMessage(field validator.FieldError) string {
	switch field.Tag() {
	case "required":
		return "is required"
	case "min":
		return fmt.Sprintf("must contain at least %s characters", field.Param())
	case "max":
		return fmt.Sprintf("must contain at most %s characters", field.Param())
	default:
		return "is invalid"
	}
}
