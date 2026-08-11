package maybe

import (
	"errors"
	"reflect"
	"strconv"
)

var ErrNilOpt = errors.New("err: nilopt")

type pncUnwrappedNilMaybe struct {
	message string
}

var PncUnwrappedNilMaybe = pncUnwrappedNilMaybe{"panic: unwrapped nil maybe"}

func (punm *pncUnwrappedNilMaybe) AllowRecover() bool {
	return false
}

type Maybe[T any] struct {
	value *T
}

func (m Maybe[T]) Raw() *T {
	return m.value
}

func (m Maybe[T]) HasValue() bool {
	return m.Raw() != nil
}

func (m Maybe[T]) Unwrap() T {
	if !m.HasValue() {
		panic(PncUnwrappedNilMaybe)
	}
	return *m.Raw()
}

func (m Maybe[T]) OrDefaultValue(defaultValue T) T {
	if !m.HasValue() {
		return defaultValue
	}
	return *m.Raw()
}

func (m Maybe[T]) OrDefault(valueProvider func() T) T {
	if !m.HasValue() {
		return valueProvider()
	}
	return *m.Raw()
}

func (m Maybe[T]) OrError() (*T, error) {
	if !m.HasValue() {
		return nil, ErrNilOpt
	}
	return m.Raw(), nil
}

func Zero[T any]() T {
	var zero T
	return zero
}

func (m Maybe[T]) OrZero() T {
	if !m.HasValue() {
		return Zero[T]()
	}
	return *m.Raw()
}

func Map[T, R any](m Maybe[T], f func(T) *R) Maybe[R] {
	if !m.HasValue() {
		return Empty[R]()
	}

	return NewMaybe(f(*m.Raw()))
}

func MapWithError[T, R any](m Maybe[T], f func(T) (R, error)) (Maybe[R], error) {
	if !m.HasValue() {
		return Empty[R](), nil
	}

	val, err := f(*m.Raw())
	if err != nil {
		return Empty[R](), err
	}

	return Some(val), nil
}

func Use[T any](m Maybe[T], f func(T) error) error {
	if !m.HasValue() {
		return nil
	}
	return f(m.Unwrap())
}

func UseAs[T any, A any](m Maybe[T], f func(A) error) error {
	usable := Map(m, func(t T) *A {
		if v, ok := any(t).(A); ok {
			return &v
		}
		return nil
	})
	return Use(usable, f)
}

func NewMaybe[T any](value *T) Maybe[T] {
	return Maybe[T]{value: value}
}

func Some[T any](value T) Maybe[T] {
	return NewMaybe(&value)
}

func Empty[T any]() Maybe[T] {
	return NewMaybe[T](nil)
}

func GetOptionalParameter[T any](optional ...T) Maybe[T] {
	if len(optional) > 0 {
		return Some(optional[0])
	}
	return Empty[T]()
}

type isZeroResult int

const (
	izYes isZeroResult = iota
	izNo
	izUnknown
)

func iz(b bool) isZeroResult {
	if b {
		return izYes
	}
	return izNo
}

func IsZero(v any) isZeroResult {
	switch v.(type) {
	case string:
		return iz(v == "")
	case int:
		return iz(v == 0)
	case float64:
		return iz(v == 0.0)
	case float32:
		return iz(v == 0.0)
	case int64:
		return iz(v == 0)
	case int32:
		return iz(v == 0)
	case int16:
		return iz(v == 0)
	case int8:
		return iz(v == 0)
	case uint:
		return iz(v == 0)
	case uint64:
		return iz(v == 0)
	case uint32:
		return iz(v == 0)
	case uint16:
		return iz(v == 0)
	case uint8:
		return iz(v == 0)
	case bool:
		return iz(v == false)
	default:
		return izUnknown
	}
}

func IsComplexZero(v any) bool {
	return reflect.ValueOf(v).IsZero()
}

func GetOptionalNonZeroParameter[T any](optional ...T) Maybe[T] {
	if len(optional) <= 0 {
		return Empty[T]()
	}

	z := IsZero(optional[0])
	if z == izYes {
		return Empty[T]()
	}

	if z == izNo {
		return Some(optional[0])
	}

	if IsComplexZero(optional[0]) {
		return Empty[T]()
	}

	return Some(optional[0])
}

func Collect[T any](maybe ...Maybe[T]) (collection []T) {
	for _, m := range maybe {
		if !m.HasValue() {
			continue
		}

		collection = append(collection, m.Unwrap())
	}

	return collection
}

func MustCollect[T any](maybe ...Maybe[T]) (collection []T) {
	for i, m := range maybe {
		if !m.HasValue() {
			panic("MustCollect: failed to collect value at index " + strconv.Itoa(i))
		}

		collection = append(collection, m.Unwrap())
	}

	return collection
}
