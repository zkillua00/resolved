package eventsystem

import (
	"errors"
	"log"

	"resolved-server/eventsystem/maybe"
)

var ErrTypeMismatch = errors.New("type mismatch")

type Event struct {
	name string
	data any
}

func Data[T any](e Event) maybe.Maybe[T] {
	return maybe.Map(maybe.Some(e.data), func(a any) *T {
		if v, ok := a.(T); ok {
			return &v
		}
		if v, ok := a.(*T); ok {
			return v
		}
		return nil
	})
}

// UseEventAs extracts the event data, tries to type assert it and run the event handler.
// if type assertion fails, the handler won't run.
func UseEventAs[A any](f func(A) error, shoudLog ...bool) func(e Event) {
	sl := maybe.GetOptionalParameter(shoudLog...).OrDefaultValue(false)
	return func(e Event) {
		if err := maybe.Use(Data[A](e), f); err != nil && sl {
			log.Printf("error in event %s: %s", e.name, err.Error())
		}
	}
}

// Name returns the name of the event.
func Name(e Event) string {
	return e.name
}

// NewEvent creates a new Event instance with the given name and generic data payload.
func NewEvent(name string, data any) Event {
	return Event{name, data}
}

type EventRegister struct {
	Name         string
	Handler      EventHandler
	Dependencies *NamedDependencyMap
}

func (e *EventRegister) RegisterSelf(listener *EventListener) {
	listener.RegisterEventHandler(e.Name, e.Handler)
}

type EventRegistry []EventRegister

func (r EventRegistry) SelfRegisterAll(eventListener *EventListener) {
	for _, reg := range r {
		reg.RegisterSelf(eventListener)
	}
}
