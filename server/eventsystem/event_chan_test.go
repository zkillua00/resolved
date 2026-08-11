package eventsystem

import (
	"testing"
)

func TestEventChan(t *testing.T) {
	ch := newEventChan(2)
	e1 := eventPair{Event: NewEvent("e1", nil)}
	e2 := eventPair{Event: NewEvent("e2", nil)}

	ch <- e1
	ch <- e2

	if len(ch) != 2 {
		t.Errorf("Expected channel length 2, got %d", len(ch))
	}

	ch.close()

	// Test if close works
	defer func() {
		if r := recover(); r == nil {
			t.Errorf("Expected panic on writing to closed channel")
		}
	}()
	ch <- eventPair{}
}
