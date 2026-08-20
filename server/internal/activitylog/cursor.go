package activitylog

import (
	"encoding/base64"
	"encoding/json"
	"time"

	"resolved-server/internal/problem"

	"github.com/google/uuid"
)

type encodedCursor struct {
	CreatedAt string `json:"created_at"`
	ID        string `json:"id"`
}

func encodeCursor(entry Entry) string {
	payload, _ := json.Marshal(encodedCursor{
		CreatedAt: entry.CreatedAt.UTC().Format(time.RFC3339Nano),
		ID:        entry.ID,
	})
	return base64.RawURLEncoding.EncodeToString(payload)
}

func decodeCursor(field, value string) (*entryCursor, error) {
	if value == "" {
		return nil, nil
	}
	payload, err := base64.RawURLEncoding.DecodeString(value)
	if err != nil {
		return nil, invalidCursor(field)
	}
	var decoded encodedCursor
	if err := json.Unmarshal(payload, &decoded); err != nil {
		return nil, invalidCursor(field)
	}
	createdAt, err := time.Parse(time.RFC3339Nano, decoded.CreatedAt)
	if err != nil || uuid.Validate(decoded.ID) != nil {
		return nil, invalidCursor(field)
	}
	return &entryCursor{CreatedAt: createdAt.UTC(), ID: decoded.ID}, nil
}

func invalidCursor(field string) error {
	return problem.WithFields(
		"validation_failed",
		"request validation failed",
		map[string]string{field: "must be a cursor returned by this endpoint"},
	)
}
