// Package validation holds small, cross-domain validation helpers shared by
// the service layer, so identity checks like "must be a valid UUID" are defined
// once instead of once per service.
package validation

import (
	"resolved-server/internal/problem"

	"github.com/google/uuid"
)

// ID validates that value is a well-formed UUID, attributing the failure to
// field.
func ID(field, value string) error {
	if _, err := uuid.Parse(value); err != nil {
		return problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{field: "must be a valid UUID"},
		)
	}
	return nil
}

// IDs validates every value in values as a UUID, attributing failures to field.
func IDs(field string, values []string) error {
	for _, value := range values {
		if err := ID(field, value); err != nil {
			return err
		}
	}
	return nil
}
