package problem

import "fmt"

type Kind uint8

const (
	KindBadRequest Kind = iota + 1
	KindInvalid
	KindUnauthorized
	KindForbidden
	KindNotFound
	KindConflict
	KindRateLimited
	KindInternal
)

type Error struct {
	Kind    Kind
	Code    string
	Message string
	Fields  map[string]string
	Cause   error
}

func (e *Error) Error() string {
	if e.Cause == nil {
		return e.Message
	}
	return fmt.Sprintf("%s: %v", e.Message, e.Cause)
}

func (e *Error) Unwrap() error {
	return e.Cause
}

func New(kind Kind, code, message string) *Error {
	return &Error{Kind: kind, Code: code, Message: message}
}

func WithFields(code, message string, fields map[string]string) *Error {
	return &Error{Kind: KindInvalid, Code: code, Message: message, Fields: fields}
}

func Wrap(cause error, message string) *Error {
	return &Error{
		Kind:    KindInternal,
		Code:    "internal_error",
		Message: message,
		Cause:   cause,
	}
}
