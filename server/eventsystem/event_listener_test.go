package eventsystem

import (
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestEventListener(t *testing.T) {
	el := NewEventListener()
	el.StartNewEventLoop()

	var wg sync.WaitGroup
	wg.Add(1)

	var executed atomic.Bool
	el.RegisterEventHandler("test_event", func(e Event) {
		executed.Store(true)
		wg.Done()
	})

	el.FireEvent("test_event", nil)

	// Wait with timeout
	waitCh := make(chan struct{})
	go func() {
		wg.Wait()
		close(waitCh)
	}()

	select {
	case <-waitCh:
	case <-time.After(1 * time.Second):
		t.Errorf("Handler was not executed in time")
	}

	if !executed.Load() {
		t.Errorf("Expected handler to be executed")
	}

	el.Stop()
	if !el.stopped() {
		t.Errorf("Expected EventListener to be stopped")
	}

	// Test operations after stop
	el.RegisterEventHandler("test_event_2", func(e Event) {})
	el.StartNewEventLoop()
	el.FireEvent("test_event_2", nil)

	// Fire an unregistered event
	el = NewEventListener()
	el.FireEvent("non_existent_event", nil)

	// Test Async Stop
	el = NewEventListener()
	el.StartNewEventLoop()
	ctx := el.StopAsync()
	<-ctx.Done()
	if !el.stopped() {
		t.Errorf("Expected EventListener to be stopped after StopAsync")
	}

	// Try stop again
	el.Stop()
}
