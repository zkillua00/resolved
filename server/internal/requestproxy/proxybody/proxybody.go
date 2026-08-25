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
	switch input.Mode {
	case "none":
		return nil, "", nil
	case "raw":
		body, err := Decode(input.DataBase64, MaxRequestBodyBytes)
		if len(body) == 0 {
			return body, "", err
		}
		return body, input.RawContentType, err
	case "form_url_encoded":
		var encoded strings.Builder
		for _, field := range input.Fields {
			if field.Kind != "text" {
				return nil, "", invalidField("body.fields", "URL-encoded fields must contain text")
			}
			if encoded.Len() > 0 {
				encoded.WriteByte('&')
			}
			encoded.WriteString(url.QueryEscape(field.Name))
			encoded.WriteByte('=')
			encoded.WriteString(url.QueryEscape(field.Value))
			if encoded.Len() > MaxRequestBodyBytes {
				return nil, "", requestBodyTooLarge()
			}
		}
		return []byte(encoded.String()), "application/x-www-form-urlencoded", nil
	case "multipart_form_data":
		var encoded bytes.Buffer
		writer := multipart.NewWriter(&encoded)
		for _, field := range input.Fields {
			switch field.Kind {
			case "text":
				if err := writer.WriteField(field.Name, field.Value); err != nil {
					return nil, "", problem.Wrap(err, "encode multipart text field")
				}
			case "file":
				content, err := Decode(field.ContentBase64, MaxRequestBodyBytes)
				if err != nil {
					return nil, "", err
				}
				part, err := createFilePart(writer, field.Name, field.Filename)
				if err != nil {
					return nil, "", problem.Wrap(err, "encode multipart file field")
				}
				if _, err := part.Write(content); err != nil {
					return nil, "", problem.Wrap(err, "encode multipart file content")
				}
			default:
				return nil, "", invalidField("body.fields", "multipart fields must contain text or file data")
			}
			if encoded.Len() > MaxRequestBodyBytes {
				return nil, "", requestBodyTooLarge()
			}
		}
		if err := writer.Close(); err != nil {
			return nil, "", problem.Wrap(err, "finish multipart body")
		}
		if encoded.Len() > MaxRequestBodyBytes {
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
	if len(encoded) > base64.StdEncoding.EncodedLen(limit) {
		return nil, requestBodyTooLarge()
	}
	decoded, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		return nil, invalidField("body", "contains invalid base64 data")
	}
	if len(decoded) > limit {
		return nil, requestBodyTooLarge()
	}
	return decoded, nil
}

func requestBodyTooLarge() error {
	return problem.New(
		problem.KindPayloadTooLarge,
		"proxy_request_too_large",
		fmt.Sprintf("the proxied request body exceeds the %d-byte limit", MaxRequestBodyBytes),
	)
}

func invalidField(field, message string) error {
	return problem.WithFields(
		"validation_failed",
		"request validation failed",
		map[string]string{field: message},
	)
}
