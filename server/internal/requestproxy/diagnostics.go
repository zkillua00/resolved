package requestproxy

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"reflect"
	"strings"

	"resolved-server/internal/problem"
)

func executionProblem(kind problem.Kind, code, message, phase, reason string, fields map[string]string) *problem.Error {
	return &problem.Error{Kind: kind, Code: code, Message: message, Phase: phase, Reason: reason, Fields: fields}
}

// Decode errors must describe the schema, never echo submitted values (including
// json.UnmarshalTypeError.Value, which can include a submitted number).
func decodeExecutionDescriptor(data []byte, destination any, phase string) error {
	trimmed := bytes.TrimSpace(data)
	field, detail, reason := "body", "expected a JSON object containing an execution descriptor", "invalid_json"
	if len(trimmed) > 0 && trimmed[0] == '{' {
		err := json.Unmarshal(data, destination)
		if err == nil {
			return nil
		}
		var mismatch *json.UnmarshalTypeError
		if errors.As(err, &mismatch) {
			// Only publish schema-defined field names, not keys from the payload.
			if path := schemaFieldPath(reflect.TypeOf(destination), mismatch.Field); path != "" {
				field = path
			}
			received := strings.SplitN(mismatch.Value, " ", 2)[0]
			switch received {
			case "array", "object", "bool", "number", "string", "null":
			default:
				received = "incompatible JSON value"
			}
			detail = fmt.Sprintf("expected %s; received %s", jsonType(mismatch.Type), received)
			reason = "type_mismatch"
		} else {
			detail = "expected a valid JSON object; received malformed JSON"
			var syntax *json.SyntaxError
			if errors.As(err, &syntax) {
				detail += fmt.Sprintf(" at byte offset %d", syntax.Offset)
			}
		}
	} else if json.Valid(data) {
		received := "number"
		switch trimmed[0] {
		case '[':
			received = "array"
		case '"':
			received = "string"
		case 't', 'f':
			received = "boolean"
		case 'n':
			received = "null"
		}
		detail = "expected a JSON object; received " + received
		reason = "type_mismatch"
	} else {
		var value any
		var syntax *json.SyntaxError
		if errors.As(json.Unmarshal(data, &value), &syntax) {
			detail = fmt.Sprintf("expected a valid JSON object; received malformed JSON at byte offset %d", syntax.Offset)
		}
	}
	return executionProblem(problem.KindBadRequest, "invalid_body", "execution descriptor is invalid", phase, reason, map[string]string{field: detail})
}

func schemaFieldPath(t reflect.Type, path string) string {
	var fields []string
	for _, name := range strings.Split(path, ".") {
		for t.Kind() == reflect.Pointer || t.Kind() == reflect.Slice || t.Kind() == reflect.Array {
			t = t.Elem()
		}
		// Map keys are submitted data, not schema-defined names.
		if t.Kind() != reflect.Struct {
			break
		}
		found := false
		for i := 0; i < t.NumField(); i++ {
			candidate := t.Field(i)
			if tag := strings.Split(candidate.Tag.Get("json"), ",")[0]; tag != "" && tag != "-" && tag == name {
				fields = append(fields, tag)
				t = candidate.Type
				found = true
				break
			}
		}
		if !found {
			break
		}
	}
	return strings.Join(fields, ".")
}

func jsonType(t reflect.Type) string {
	switch t.Kind() {
	case reflect.String:
		return "string"
	case reflect.Bool:
		return "boolean"
	case reflect.Slice, reflect.Array:
		return "array"
	case reflect.Struct, reflect.Map:
		return "object"
	default:
		return "a compatible JSON value"
	}
}
