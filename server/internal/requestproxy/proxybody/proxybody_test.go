package proxybody

import (
	"encoding/base64"
	"errors"
	"strings"
	"testing"

	"resolved-server/internal/problem"
)

func TestBuildWithLimitAllEncodings(t *testing.T) {
	for _, input := range []Body{
		{Mode: "raw", DataBase64: base64.StdEncoding.EncodeToString([]byte("hello"))},
		{Mode: "form_url_encoded", Fields: []BodyField{{Kind: "text", Name: "name", Value: "hello & world"}}},
		{Mode: "multipart_form_data", Fields: []BodyField{{Kind: "text", Name: "name", Value: "hello"}}},
		{Mode: "multipart_form_data", Fields: []BodyField{{Kind: "file", Name: "file", Filename: "a.txt", ContentBase64: base64.StdEncoding.EncodeToString([]byte("hello"))}}},
	} {
		t.Run(input.Mode, func(t *testing.T) {
			body, _, err := BuildWithLimit(input, Limit{Unlimited: true})
			if err != nil {
				t.Fatal(err)
			}
			for _, cap := range []int64{0, 1, int64(len(body) - 1)} {
				_, _, err := BuildWithLimit(input, Limit{Value: cap})
				var p *problem.Error
				if !errors.As(err, &p) || p.Kind != problem.KindPayloadTooLarge {
					t.Fatalf("cap %d: %v", cap, err)
				}
			}
			if _, _, err := BuildWithLimit(input, Limit{Value: int64(len(body))}); err != nil {
				t.Fatal(err)
			}
		})
	}
}

func TestBoundedWriterDoesNotGrowPastLimit(t *testing.T) {
	writer := boundedBuffer{limit: Limit{Value: 2}}
	if _, err := writer.WriteString(strings.Repeat("x", 100)); err == nil {
		t.Fatal("large write accepted")
	}
	if writer.Len() != 0 {
		t.Fatal("oversized write allocated body storage")
	}
}
