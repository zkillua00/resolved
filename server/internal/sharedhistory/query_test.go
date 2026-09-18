package sharedhistory

import (
	"slices"
	"strings"
	"testing"
	"time"
)

func TestHistoryQueryValidation(t *testing.T) {
	for _, input := range []ListOptions{
		{Method: "GET POST"}, {Method: strings.Repeat("X", 65)}, {Method: "GÉT"},
		{Status: "600"}, {Status: "99"}, {Status: "6xx"}, {Status: "wat"},
		{BodyType: "json"}, {BodyType: "raw:unknown"}, {Sort: "random"},
		{From: "yesterday"}, {Before: "2026-01-01"},
		{From: "2026-01-02T00:00:00Z", Before: "2026-01-01T00:00:00Z"},
		{From: "2026-01-01T00:00:00Z", Before: "2026-01-01T00:00:00Z"},
		{HeaderKeys: strings.Repeat("a,", 257)}, {ParamKeys: strings.Repeat("x", 4097)},
	} {
		if _, err := parseHistoryQuery(input); err == nil {
			t.Errorf("accepted invalid options %+v", input)
		}
	}
}

func TestHistoryQueryRepeatedParamsAndEffectiveFilters(t *testing.T) {
	entry := Entry{URL: "https://example.test/?first=1&first=2&second=&escaped%20key=3"}
	for _, keys := range []string{"first,second,escaped key", "first,first,second"} {
		q, err := parseHistoryQuery(ListOptions{ParamKeys: keys})
		if err != nil {
			t.Fatal(err)
		}
		if matched, err := q.matches(entry); err != nil || !matched {
			t.Fatalf("keys %q: matched=%v err=%v", keys, matched, err)
		}
	}
	for _, input := range []ListOptions{{}, {Sort: "newest"}, {Sort: "newest", HeaderKeys: ", ,", ParamKeys: ", ,"}} {
		q, err := parseHistoryQuery(input)
		if err != nil || !q.unfilteredNewest() {
			t.Fatalf("not effective newest: %+v, %v", input, err)
		}
	}
	for _, input := range []ListOptions{{Sort: "oldest"}, {ParamKeys: "key"}, {Method: "GET"}, {Path: "/"}} {
		q, err := parseHistoryQuery(input)
		if err != nil || q.unfilteredNewest() {
			t.Fatalf("ignored effective filter: %+v, %v", input, err)
		}
	}
}

func BenchmarkHistoryQueryRepeatedParams(b *testing.B) {
	q, err := parseHistoryQuery(ListOptions{ParamKeys: strings.Repeat("key,", 256)})
	if err != nil {
		b.Fatal(err)
	}
	entry := Entry{URL: "https://example.test/?" + strings.Repeat("key=value&", 256)}
	b.ReportAllocs()
	for b.Loop() {
		if matched, err := q.matches(entry); err != nil || !matched {
			b.Fatal(matched, err)
		}
	}
}

func TestHistoryQueryANDAndRequestOnlySemantics(t *testing.T) {
	status := 201
	entry := Entry{
		Method: "POST", URL: "https://API.Example.test:8443/Widgets?first%20key=x&second=needle",
		RequestHeadersJSON:  []byte(`[{"name":"X-One","value":"value"},{"name":"X-Two","value":"secret"}]`),
		ResponseHeadersJSON: []byte(`[{"name":"Response-Only","value":"yes"}]`),
		RequestBodyMode:     "raw", RequestBodyLanguage: "json", ResponseStatus: &status,
		CreatedAt: time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
	}
	options := ListOptions{
		Method: "post", Status: "2xx", Hostname: "api.example", Path: "/Wid",
		HeaderKeys: " x-one, ,X-TWO, ", ParamKeys: "first key,second",
		BodyType: "raw:json", From: "2026-01-01T00:00:00Z", Before: "2026-01-02T00:00:00Z",
	}
	assertMatch := func(options ListOptions, want bool) {
		t.Helper()
		q, err := parseHistoryQuery(options)
		if err != nil {
			t.Fatal(err)
		}
		if got, err := q.matches(entry); err != nil || got != want {
			t.Fatalf("options %+v: match=%v err=%v want=%v", options, got, err, want)
		}
	}
	assertMatch(options, true)
	options.ParamKeys += ",missing"
	assertMatch(options, false)
	for _, input := range []ListOptions{
		{Hostname: "8443"}, {Hostname: "Widgets"}, {Path: "needle"}, {Path: "/wid"},
		{HeaderKeys: "Response-Only"}, {HeaderKeys: "value"}, {ParamKeys: "needle"},
		{ParamKeys: "First key"}, {BodyType: "raw:xml"}, {Status: "200"}, {Status: "error"},
		{Before: "2026-01-01T00:00:00Z"},
	} {
		assertMatch(input, false)
	}
	assertMatch(ListOptions{BodyType: "raw", Status: "201"}, true)
	entry.ResponseStatus = nil
	assertMatch(ListOptions{Status: "error"}, true)
}

func TestHistorySortsAndDeterministicTies(t *testing.T) {
	ok, failed := 200, 500
	short, long := int64(10), int64(20)
	now := time.Now().UTC()
	entries := []Entry{
		{ID: "a", CreatedAt: now, Method: "GET", URL: "https://a.test/a", ResponseStatus: &ok, ResponseDurationMicros: &long},
		{ID: "b", CreatedAt: now.Add(time.Second), Method: "POST", URL: "https://b.test/b", ResponseStatus: &failed, ResponseDurationMicros: &short},
		{ID: "c", CreatedAt: now.Add(2 * time.Second), Method: "PUT", URL: "https://c.test/c"},
	}
	for sort, want := range map[string]string{
		"newest": "cba", "oldest": "abc", "status_asc": "abc", "status_desc": "bac",
		"duration_asc": "bac", "duration_desc": "abc", "method": "abc", "hostname": "abc", "path": "abc",
	} {
		q, err := parseHistoryQuery(ListOptions{Sort: sort})
		if err != nil {
			t.Fatal(err)
		}
		got := slices.Clone(entries)
		slices.SortFunc(got, q.compare)
		if ids := got[0].ID + got[1].ID + got[2].ID; ids != want {
			t.Errorf("%s: got %s want %s", sort, ids, want)
		}
		a, b := entries[0], entries[0]
		b.ID = "z"
		wantOrder := -1
		if sort != "oldest" {
			wantOrder = 1
		}
		if q.compare(a, b) != wantOrder {
			t.Errorf("%s: ID tie breaker incorrect", sort)
		}
	}
}
