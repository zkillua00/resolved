// Package proxybody encodes a request body for the Resolved request proxy:
// raw/form/multipart payload construction and the shared body size limit.
//
// Kept separate from the proxy Service so the encoding concern can change
// without touching request execution, and reused wherever a body must be
// validated or built.
package proxybody

import (
	"bytes"
	"encoding/base64"
	"fmt"
	"io"
	"mime/multipart"
	"net/textproto"
	"net/url"
	"path/filepath"
	"strings"

	"resolved-server/internal/problem"
)

const MaxRequestBodyBytes = 64 * 1024 * 1024

type Limit struct {
	Unlimited bool
	Value     int64
}

// Body describes a request body to encode, matching the wire format each entry
// in History and the proxy requests accept.
type Body struct {
	Mode           string      `json:"mode"`
	RawContentType string      `json:"raw_content_type,omitempty"`
	DataBase64     string      `json:"data_base64,omitempty"`
	Fields         []BodyField `json:"fields,omitempty"`
}

type BodyField struct {
	Name          string `json:"name"`
	Kind          string `json:"kind"`
	Value         string `json:"value,omitempty"`
	Filename      string `json:"filename,omitempty"`
	ContentBase64 string `json:"content_base64,omitempty"`
}

// Build encodes input into a request body and its Content-Type, applying the
// request size limit throughout.
func Build(input Body) ([]byte, string, error) {
	return BuildWithLimit(input, Limit{Value: MaxRequestBodyBytes})
}

func BuildWithLimit(input Body, limit Limit) ([]byte, string, error) {
	switch input.Mode {
	case "none":
		return nil, "", nil
	case "raw":
		body, err := decodeBound(input.DataBase64, limit)
		if len(body) == 0 {
			return body, "", err
		}
		return body, input.RawContentType, err
	case "form_url_encoded":
		encoded := boundedBuffer{limit: limit}
		for _, field := range input.Fields {
			if field.Kind != "text" {
				return nil, "", invalidField("body.fields", "URL-encoded fields must contain text")
			}
			if !limit.Unlimited {
				needed := queryEncodedLen(field.Name) + queryEncodedLen(field.Value) + 1
				if encoded.Len() > 0 {
					needed++
				}
				if needed > limit.Value-int64(encoded.Len()) {
					return nil, "", requestBodyTooLarge()
				}
			}
			if encoded.Len() > 0 {
				encoded.WriteByte('&')
			}
			encoded.WriteString(url.QueryEscape(field.Name))
			encoded.WriteByte('=')
			encoded.WriteString(url.QueryEscape(field.Value))
			if encoded.exceeded {
				return nil, "", requestBodyTooLarge()
			}
		}
		return []byte(encoded.String()), "application/x-www-form-urlencoded", nil
	case "multipart_form_data":
		encoded := boundedBuffer{limit: limit}
		writer := multipart.NewWriter(&encoded)
		for _, field := range input.Fields {
			switch field.Kind {
			case "text":
				if err := writer.WriteField(field.Name, field.Value); err != nil {
					if encoded.exceeded {
						return nil, "", requestBodyTooLarge()
					}
					return nil, "", problem.Wrap(err, "encode multipart text field")
				}
			case "file":
				content, err := decodeBound(field.ContentBase64, limit)
				if err != nil {
					return nil, "", err
				}
				part, err := createFilePart(writer, field.Name, field.Filename)
				if err != nil {
					if encoded.exceeded {
						return nil, "", requestBodyTooLarge()
					}
					return nil, "", problem.Wrap(err, "encode multipart file field")
				}
				if _, err := part.Write(content); err != nil {
					if encoded.exceeded {
						return nil, "", requestBodyTooLarge()
					}
					return nil, "", problem.Wrap(err, "encode multipart file content")
				}
			default:
				return nil, "", invalidField("body.fields", "multipart fields must contain text or file data")
			}
			if encoded.exceeded {
				return nil, "", requestBodyTooLarge()
			}
		}
		if err := writer.Close(); err != nil {
			if encoded.exceeded {
				return nil, "", requestBodyTooLarge()
			}
			return nil, "", problem.Wrap(err, "finish multipart body")
		}
		if encoded.exceeded {
			return nil, "", requestBodyTooLarge()
		}
		return encoded.Bytes(), writer.FormDataContentType(), nil
	default:
		return nil, "", invalidField("body.mode", "is invalid")
	}
}

func createFilePart(writer *multipart.Writer, fieldName, filename string) (io.Writer, error) {
	header := make(textproto.MIMEHeader)
	header.Set(
		"Content-Disposition",
		fmt.Sprintf(`form-data; name=%q; filename=%q`, escapeQuotes(fieldName), escapeQuotes(filename)),
	)
	contentType := "application/octet-stream"
	if detected := mimeTypeForFilename(filename); detected != "" {
		contentType = detected
	}
	header.Set("Content-Type", contentType)
	return writer.CreatePart(header)
}

func mimeTypeForFilename(filename string) string {
	extension := strings.ToLower(filepath.Ext(filename))
	switch extension {
	case ".json":
		return "application/json"
	case ".xml":
		return "application/xml"
	case ".html", ".htm":
		return "text/html"
	case ".txt", ".md", ".csv":
		return "text/plain"
	case ".png":
		return "image/png"
	case ".jpg", ".jpeg":
		return "image/jpeg"
	case ".gif":
		return "image/gif"
	case ".pdf":
		return "application/pdf"
	default:
		return ""
	}
}

func escapeQuotes(value string) string {
	return strings.NewReplacer("\\", "\\\\", `"`, `\"`, "\r", "", "\n", "").Replace(value)
}

// Decode validates and decodes a base64 request body, enforcing limit.
func Decode(encoded string, limit int) ([]byte, error) {
	return decodeBound(encoded, Limit{Value: int64(limit)})
}

func decodeBound(encoded string, limit Limit) ([]byte, error) {
	var reader io.Reader = base64.NewDecoder(base64.StdEncoding, strings.NewReader(encoded))
	if !limit.Unlimited && limit.Value < int64(^uint64(0)>>1) {
		reader = io.LimitReader(reader, limit.Value+1)
	}
	decoded, err := io.ReadAll(reader)
	if err != nil {
		return nil, invalidField("body", "contains invalid base64 data")
	}
	if !limit.Unlimited && int64(len(decoded)) > limit.Value {
		return nil, requestBodyTooLarge()
	}
	return decoded, nil
}

// Reject writes before growing the encoded body, including multipart headers.
type boundedBuffer struct {
	bytes.Buffer
	limit    Limit
	exceeded bool
}

func (b *boundedBuffer) Write(p []byte) (int, error) {
	if !b.limit.Unlimited && int64(len(p)) > b.limit.Value-int64(b.Len()) {
		b.exceeded = true
		return 0, requestBodyTooLarge()
	}
	return b.Buffer.Write(p)
}
func (b *boundedBuffer) WriteString(value string) (int, error) {
	if !b.limit.Unlimited && int64(len(value)) > b.limit.Value-int64(b.Len()) {
		b.exceeded = true
		return 0, requestBodyTooLarge()
	}
	return b.Buffer.WriteString(value)
}
func (b *boundedBuffer) WriteByte(value byte) error { _, err := b.Write([]byte{value}); return err }

func queryEncodedLen(value string) int64 {
	var size int64
	for i := 0; i < len(value); i++ {
		c := value[i]
		if c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '-' || c == '_' || c == '.' || c == '~' || c == ' ' {
			size++
		} else {
			size += 3
		}
	}
	return size
}

func requestBodyTooLarge() error {
	return problem.New(
		problem.KindPayloadTooLarge,
		"proxy_request_too_large",
		"the proxied request body exceeds its execution limit",
	)
}

func invalidField(field, message string) error {
	return problem.WithFields(
		"validation_failed",
		"request validation failed",
		map[string]string{field: message},
	)
}
