package eventsystem

import (
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestEventLoop(t *testing.T) {
	ch := newEventChan(10)
	el := NewEventLoop()

	var wg sync.WaitGroup
	wg.Add(1)

	go func() {
		defer wg.Done()
		el.eventLoop(ch, 0)
	}()

	var executed atomic.Bool
	handler := func(e Event) {
		executed.Store(true)
	}

	ch <- eventPair{
		Event:        NewEvent("test", nil),
		EventHandler: []EventHandler{handler},
	}

	// Wait a bit to ensure loop processes it
	time.Sleep(10 * time.Millisecond)

	if !executed.Load() {
		t.Errorf("Expected handler to be executed")
	}

	// Test panic recovery
	var panicExecuted atomic.Bool
	panicHandler := func(e Event) {
		panicExecuted.Store(true)
		panic("test panic")
	}

	ch <- eventPair{
		Event:        NewEvent("panic", nil),
		EventHandler: []EventHandler{panicHandler},
	}
	ch <- eventPair{
		Event:        NewEvent("panic", nil),
		EventHandler: []EventHandler{panicHandler},
	}
	ch <- eventPair{
		Event:        NewEvent("panic", nil),
		EventHandler: []EventHandler{panicHandler},
	}

	time.Sleep(10 * time.Millisecond)

	if !panicExecuted.Load() {
		t.Errorf("Expected panic handler to be executed")
	}

	// Test stop
	els := NewEventLoops()
	els.AddLoop(el)
	els.Stop()

	// Wait for event loop to exit
	wg.Wait()
}
