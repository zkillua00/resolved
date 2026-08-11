package eventsystem

import (
	"log"
	"sync"
)

type EventLoop struct {
	loop     func(eventChan)
	stopChan chan struct{}
}

// NewEventLoop initializes an EventLoop structurally but doesn't start its execution.
func NewEventLoop() *EventLoop {
	el := &EventLoop{
		stopChan: make(chan struct{}),
	}
	el.loop = func(ec eventChan) { el.eventLoop(ec, 0) }
	return el
}

// runEvent executes all provided handlers synchronously for the given event.
// Any panic within a handler will escalate to the caller (eventLoop).
func runEvent(event Event, handlers []EventHandler) {
	for _, handler := range handlers {
		handler(event)
	}
}

// eventLoop consistently reads from the eventChannel, executing events until stopChan is closed.
// Recovers from panics up to 2 times attempting to revive the loop; subsequent panics are unhandled or fail to revive.
func (el *EventLoop) eventLoop(eventChannel eventChan, rerunCount int) {
	var lastEventName string
	defer func() {
		if v := recover(); v != nil {
			log.Printf("[ERROR] recovered from panic with '%v' in event '%s'", v, lastEventName)
			if rerunCount < 2 {
				go el.eventLoop(eventChannel, rerunCount+1) // try to save the routine
			} else {
				log.Printf("failed to save the goroutine")
			}
		}

	}()

	stopRequested := false
	for !stopRequested {
		select {
		case <-el.stopChan:
			stopRequested = true
		case ep, ok := <-eventChannel:
			if !ok {
				return
			}
			lastEventName = ep.Event.name
			runEvent(ep.Event, ep.EventHandler)
		}
	}
}

type EventLoops struct {
	loops []*EventLoop
	mu    sync.Mutex
}

// NewEventLoops initializes a collection to manage multiple concurrent EventLoops.
func NewEventLoops() *EventLoops {
	return &EventLoops{
		loops: make([]*EventLoop, 0),
		mu:    sync.Mutex{},
	}
}

// AddLoop appends a dynamically created loop to the collection safely.
// Locks the internal mutex; do not call in tight, performance-critical paths without necessity.
func (e *EventLoops) AddLoop(el *EventLoop) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.loops = append(e.loops, el)
}

// Stop sends stop signals to all managed EventLoops by closing their channels.
// Locks the mutex; does not wait for loops to fully exit, only signals them.
func (e *EventLoops) Stop() {
	e.mu.Lock()
	defer e.mu.Unlock()

	for _, loop := range e.loops {
		close(loop.stopChan)
	}
}
