package workspaces

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestRequestDefinitionDiffsAreFieldLevelAndRedactProtectedHeaders(t *testing.T) {
	before := `{
		"request": {
			"method": "GET",
			"body": "old-secret before",
			"headers": [
				{"enabled": true, "shared": false, "name": "X-Partner-Token", "value": "old-secret"}
			],
			"body_fields": []
		}
	}`
	after := `{
		"request": {
			"method": "POST",
			"body": "new-secret after",
			"headers": [
				{"enabled": true, "shared": false, "name": "X-Partner-Token", "value": "new-secret"}
			],
			"body_fields": [{"kind": "file", "name": "upload", "value": "/private/new.txt"}]
		}
	}`

	diffs := requestDefinitionDiffs(before, after)
	encoded, err := json.Marshal(diffs)
	if err != nil {
		t.Fatalf("marshal diffs: %v", err)
	}
	serialized := string(encoded)
	for _, forbidden := range []string{"old-secret", "new-secret", "/private/old.txt", "/private/new.txt"} {
		if strings.Contains(serialized, forbidden) {
			t.Fatalf("protected value %q leaked into request diff: %s", forbidden, serialized)
		}
	}
	if !strings.Contains(serialized, `"field":"definition.request.method"`) ||
		!strings.Contains(serialized, `"from":"GET"`) ||
		!strings.Contains(serialized, `"to":"POST"`) {
		t.Fatalf("method before/after diff is missing: %s", serialized)
	}
	if !strings.Contains(serialized, logRedactedValue) ||
		!strings.Contains(serialized, "[FILE OMITTED]") {
		t.Fatalf("protected request values were not represented safely: %s", serialized)
	}
}
