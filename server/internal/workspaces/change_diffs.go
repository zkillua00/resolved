package workspaces

import (
	"encoding/json"
	"reflect"
	"sort"
	"strconv"
	"strings"

	"resolved-server/internal/resourceevents"
)

const logRedactedValue = "[REDACTED]"

func sortedLogStrings(values []string) []string {
	result := append([]string(nil), values...)
	sort.Strings(result)
	return result
}

func requestDefinitionDiffs(before, after string) []resourceevents.Diff {
	var beforeValue, afterValue any
	if err := json.Unmarshal([]byte(before), &beforeValue); err != nil {
		beforeValue = before
	}
	if err := json.Unmarshal([]byte(after), &afterValue); err != nil {
		afterValue = after
	}
	secrets := make([]string, 0)
	collectDefinitionSecrets(beforeValue, &secrets)
	collectDefinitionSecrets(afterValue, &secrets)
	beforeValue = sanitizeDefinitionValue(beforeValue, secrets)
	afterValue = sanitizeDefinitionValue(afterValue, secrets)
	diffs := make([]resourceevents.Diff, 0)
	diffJSONValues("definition", beforeValue, afterValue, &diffs)
	return diffs
}

func requestDefinitionCreatedDiff(definition string) resourceevents.Diff {
	diffs := requestDefinitionDiffs("null", definition)
	if len(diffs) == 1 && diffs[0].Field == "definition" {
		return diffs[0]
	}
	var value any
	_ = json.Unmarshal([]byte(definition), &value)
	secrets := make([]string, 0)
	collectDefinitionSecrets(value, &secrets)
	return resourceevents.Diff{
		Field: "definition",
		From:  nil,
		To:    sanitizeDefinitionValue(value, secrets),
	}
}

func collectDefinitionSecrets(value any, result *[]string) {
	switch value := value.(type) {
	case map[string]any:
		for key, nested := range value {
			if strings.EqualFold(key, "headers") {
				if headers, ok := nested.([]any); ok {
					for _, candidate := range headers {
						header, ok := candidate.(map[string]any)
						if !ok {
							continue
						}
						name, _ := header["name"].(string)
						shared, hasShared := header["shared"].(bool)
						secret, _ := header["value"].(string)
						if secret != "" && (sensitiveHeaderName(name) || hasShared && !shared) {
							*result = append(*result, secret)
						}
					}
				}
			}
			collectDefinitionSecrets(nested, result)
		}
	case []any:
		for _, nested := range value {
			collectDefinitionSecrets(nested, result)
		}
	}
}

func sanitizeDefinitionValue(value any, secrets []string) any {
	switch value := value.(type) {
	case string:
		for _, secret := range secrets {
			if secret != "" {
				value = strings.ReplaceAll(value, secret, logRedactedValue)
			}
		}
		return value
	case []any:
		result := make([]any, len(value))
		for index, nested := range value {
			result[index] = sanitizeDefinitionValue(nested, secrets)
		}
		return result
	case map[string]any:
		result := make(map[string]any, len(value))
		for key, nested := range value {
			result[key] = sanitizeDefinitionValue(nested, secrets)
		}
		if kind, _ := value["kind"].(string); kind == "file" {
			if _, exists := result["value"]; exists {
				result["value"] = "[FILE OMITTED]"
			}
		}
		return result
	default:
		return value
	}
}

func diffJSONValues(path string, before, after any, result *[]resourceevents.Diff) {
	if reflect.DeepEqual(before, after) {
		return
	}
	beforeMap, beforeIsMap := before.(map[string]any)
	afterMap, afterIsMap := after.(map[string]any)
	if beforeIsMap && afterIsMap {
		keys := make(map[string]struct{}, len(beforeMap)+len(afterMap))
		for key := range beforeMap {
			keys[key] = struct{}{}
		}
		for key := range afterMap {
			keys[key] = struct{}{}
		}
		ordered := make([]string, 0, len(keys))
		for key := range keys {
			ordered = append(ordered, key)
		}
		sort.Strings(ordered)
		for _, key := range ordered {
			diffJSONValues(path+"."+key, beforeMap[key], afterMap[key], result)
		}
		return
	}
	beforeList, beforeIsList := before.([]any)
	afterList, afterIsList := after.([]any)
	if beforeIsList && afterIsList {
		length := max(len(beforeList), len(afterList))
		for index := 0; index < length; index++ {
			var beforeItem, afterItem any
			if index < len(beforeList) {
				beforeItem = beforeList[index]
			}
			if index < len(afterList) {
				afterItem = afterList[index]
			}
			diffJSONValues(path+"["+strconv.Itoa(index)+"]", beforeItem, afterItem, result)
		}
		return
	}
	*result = append(*result, resourceevents.Diff{Field: path, From: before, To: after})
}

func sensitiveHeaderName(name string) bool {
	switch strings.ToLower(strings.TrimSpace(name)) {
	case "authorization", "proxy-authorization", "cookie", "set-cookie",
		"x-api-key", "x-auth-token", "api-key", "www-authenticate", "proxy-authenticate":
		return true
	default:
		return false
	}
}
