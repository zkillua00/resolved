package sharedhistory

import (
	"cmp"
	"encoding/json"
	"net/url"
	"slices"
	"strconv"
	"strings"
	"time"
)

// ListOptions are optional URL query values. The zero value preserves the
// newest-first listing; filters are combined with AND.
type ListOptions struct {
	Method, Status, Hostname, Path, HeaderKeys, ParamKeys, BodyType, From, Before, Sort string
}

type historyQuery struct {
	ListOptions
	headers, params []string
	from, before    time.Time
	status          int
}

func parseHistoryQuery(options ListOptions) (historyQuery, error) {
	q := historyQuery{ListOptions: options}
	for field, value := range map[string]string{
		"method": q.Method, "status": q.Status, "hostname": q.Hostname,
		"path": q.Path, "header_keys": q.HeaderKeys, "param_keys": q.ParamKeys,
		"body_type": q.BodyType, "from": q.From, "before": q.Before, "sort": q.Sort,
	} {
		if len(value) > 4096 {
			return q, invalidField(field, "must not exceed 4096 bytes")
		}
	}
	if len(q.Method) > 64 || (q.Method != "" && !httpToken(q.Method)) {
		return q, invalidField("method", "must be one HTTP method token of at most 64 bytes")
	}
	if q.Status != "" && q.Status != "error" {
		if len(q.Status) == 3 && q.Status[0] >= '1' && q.Status[0] <= '5' && q.Status[1:] == "xx" {
			q.status = int(q.Status[0]-'0') * 100
		} else {
			status, err := strconv.Atoi(q.Status)
			if err != nil || len(q.Status) != 3 || status < 100 || status > 599 {
				return q, invalidField("status", "must be 100..599, 1xx..5xx, or error")
			}
			q.status = status
		}
	}
	q.headers = queryKeys(q.HeaderKeys)
	q.params = queryKeys(q.ParamKeys)
	if len(q.headers) > 256 || len(q.params) > 256 {
		return q, invalidField("header_keys/param_keys", "must contain at most 256 names")
	}
	if q.BodyType != "" && !validBodyMode(q.BodyType) {
		language, raw := strings.CutPrefix(q.BodyType, "raw:")
		if !raw || !slices.Contains(strings.Fields("text json jsonl xml html javascript typescript css markdown graphql yaml toml sql shell rust python"), language) {
			return q, invalidField("body_type", "must be a body mode or raw:<language>")
		}
	}
	for _, bound := range []struct {
		name, value string
		target      *time.Time
	}{{"from", q.From, &q.from}, {"before", q.Before, &q.before}} {
		if bound.value != "" {
			parsed, err := time.Parse(time.RFC3339, bound.value)
			if err != nil {
				return q, invalidField(bound.name, "must be RFC3339")
			}
			*bound.target = parsed
		}
	}
	if q.From != "" && q.Before != "" && !q.from.Before(q.before) {
		return q, invalidField("before", "must be later than from")
	}
	if !slices.Contains([]string{"", "newest", "oldest", "status_asc", "status_desc", "duration_asc", "duration_desc", "method", "hostname", "path"}, q.Sort) {
		return q, invalidField("sort", "unsupported history sort")
	}
	return q, nil
}

func httpToken(value string) bool {
	for _, c := range value {
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || strings.ContainsRune("!#$%&'*+-.^_`|~", c)) {
			return false
		}
	}
	return value != ""
}

func queryKeys(value string) []string {
	var keys []string
	for _, key := range strings.Split(value, ",") {
		if key = strings.TrimSpace(key); key != "" {
			keys = append(keys, key)
		}
	}
	return keys
}

func requestURL(entry Entry) *url.URL {
	parsed, err := url.Parse(entry.URL)
	if err != nil {
		return &url.URL{}
	}
	return parsed
}

func (q historyQuery) unfilteredNewest() bool {
	return (q.Sort == "" || q.Sort == "newest") &&
		q.Method == "" && q.Status == "" && q.Hostname == "" && q.Path == "" &&
		len(q.headers) == 0 && len(q.params) == 0 && q.BodyType == "" &&
		q.From == "" && q.Before == ""
}

func (q historyQuery) matches(entry Entry) (bool, error) {
	if q.Method != "" && !strings.EqualFold(q.Method, entry.Method) ||
		q.From != "" && entry.CreatedAt.Before(q.from) ||
		q.Before != "" && !entry.CreatedAt.Before(q.before) {
		return false, nil
	}
	if q.Status == "error" {
		if entry.ResponseStatus != nil {
			return false, nil
		}
	} else if q.Status != "" {
		if entry.ResponseStatus == nil {
			return false, nil
		}
		if strings.HasSuffix(q.Status, "xx") {
			if *entry.ResponseStatus/100 != q.status/100 {
				return false, nil
			}
		} else if *entry.ResponseStatus != q.status {
			return false, nil
		}
	}
	parsed := requestURL(entry)
	if !strings.Contains(strings.ToLower(parsed.Hostname()), strings.ToLower(q.Hostname)) ||
		!strings.Contains(parsed.Path, q.Path) {
		return false, nil
	}
	if len(q.params) > 0 {
		params := parsed.Query()
		for _, key := range q.params {
			if !params.Has(key) {
				return false, nil
			}
		}
	}
	if len(q.headers) > 0 {
		var headers []Header
		if err := json.Unmarshal(entry.RequestHeadersJSON, &headers); err != nil {
			return false, err
		}
		for _, key := range q.headers {
			if !slices.ContainsFunc(headers, func(header Header) bool { return strings.EqualFold(key, header.Name) }) {
				return false, nil
			}
		}
	}
	if q.BodyType != "" {
		mode, language, rawLanguage := strings.Cut(q.BodyType, ":")
		if mode != entry.RequestBodyMode || rawLanguage && language != entry.RequestBodyLanguage {
			return false, nil
		}
	}
	return true, nil
}

func (q historyQuery) compare(a, b Entry) int {
	var order int
	switch q.Sort {
	case "oldest":
		if order = a.CreatedAt.Compare(b.CreatedAt); order == 0 {
			order = strings.Compare(a.ID, b.ID)
		}
		return order
	case "status_asc", "status_desc", "duration_asc", "duration_desc":
		duration := strings.HasPrefix(q.Sort, "duration")
		aMissing := a.ResponseStatus == nil || duration && a.ResponseDurationMicros == nil
		bMissing := b.ResponseStatus == nil || duration && b.ResponseDurationMicros == nil
		if aMissing != bMissing {
			if aMissing {
				return 1
			}
			return -1
		}
		if !aMissing {
			if duration {
				order = cmp.Compare(*a.ResponseDurationMicros, *b.ResponseDurationMicros)
			} else {
				order = cmp.Compare(*a.ResponseStatus, *b.ResponseStatus)
			}
			if strings.HasSuffix(q.Sort, "_desc") {
				order = -order
			}
		}
	case "method":
		order = strings.Compare(strings.ToUpper(a.Method), strings.ToUpper(b.Method))
	case "hostname":
		order = strings.Compare(strings.ToLower(requestURL(a).Hostname()), strings.ToLower(requestURL(b).Hostname()))
	case "path":
		order = strings.Compare(requestURL(a).Path, requestURL(b).Path)
	}
	if order == 0 {
		order = b.CreatedAt.Compare(a.CreatedAt)
	}
	if order == 0 {
		order = strings.Compare(b.ID, a.ID)
	}
	return order
}
