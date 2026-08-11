package eventsystem

import (
	"context"
	"runtime"
	"slices"
	"sync"
	"sync/atomic"

	"github.com/puzpuzpuz/xsync/v4"
)

const stoppedBit uint64 = 1 << 63
const countMask uint64 = ^stoppedBit

type EventHandler func(Event)

type ctxWrapper struct {
	ctx    context.Context
	cancel func()
}

// EventListener manages the registration and dispatching of events to their respective handlers.
// NOTE: Handlers for a specific event execute in the order they were registered.
// However, the overall processing order of events is not guaranteed.
// If multiple events are triggered, their handlers could run concurrently across loops,
// so the second handler of Event A might finish later than the first handler of Event B.
type EventListener struct {
	eventHandlers *xsync.Map[string, []EventHandler]
	eventLoopChan eventChan

	state *atomic.Uint64

	eventLoops *EventLoops
	wg         *sync.WaitGroup
	stopCtx    *atomic.Pointer[ctxWrapper]
}

// NewEventListener initializes a fresh EventListener with its internal structures and event channel.
// Keep in mind that while handlers for a specific event execute sequentially,
// the processing order of the actual events across loops is not guaranteed.
func NewEventListener() *EventListener {
	return &EventListener{
		eventHandlers: xsync.NewMap[string, []EventHandler](),
		eventLoopChan: newEventChan(64),

		state: &atomic.Uint64{},

		eventLoops: NewEventLoops(),
		wg:         &sync.WaitGroup{},
		stopCtx:    &atomic.Pointer[ctxWrapper]{},
	}
}

// tryEnterFire attempts to increment the count of active senders if the listener is not stopped.
// Panics if the active sender count overflows the countMask. Returns false if the listener is stopped.
func (e *EventListener) tryEnterFire() bool {
	for {
		s := e.state.Load()
		if s&stoppedBit != 0 {
			return false
		}
		if s&countMask == countMask {
			panic("sender count overflow")
		}
		if e.state.CompareAndSwap(s, s+1) {
			return true
		}
	}
}

// leaveFire decrements the active sender count.
// Must be called exactly once for every successful tryEnterFire to prevent blocking during Stop().
func (e *EventListener) leaveFire() {
	e.state.Add(^uint64(0)) // -1
}

// Fire dispatches the full Event to all registered handlers for the event's name.
// Does not guarantee immediate execution; handlers are dispatched to the event loops concurrently.
func (e *EventListener) Fire(event Event) {
	handlers, ok := e.eventHandlers.Load(Name(event))
	if !ok {
		return
	}

	e.fireEvent(event, slices.Clone(handlers))
}

// FireEvent creates an Event and dispatches it by calling Fire.
func (e *EventListener) FireEvent(name string, data any) {
	e.Fire(NewEvent(name, data))
}

// RegisterEventHandler adds a new handler function for the specified event name.
// Thread-safe, but will silently be ignored if called after the listener has been stopped.
func (e *EventListener) RegisterEventHandler(eventName string, eventHandler EventHandler) {
	if e.stopped() {
		return
	}
	e.eventHandlers.Compute(eventName, func(oldValue []EventHandler, loaded bool) (newValue []EventHandler, op xsync.ComputeOp) {
		if !loaded {
			return []EventHandler{eventHandler}, xsync.UpdateOp
		}
		oldValue = append(oldValue, eventHandler)
		return oldValue, xsync.UpdateOp
	})
}

func (e *EventListener) RegisterEventHandlers(eventName string, eventHandlers ...EventHandler) {
	for _, eventHandler := range eventHandlers {
		e.RegisterEventHandler(eventName, eventHandler)
	}
}

// func (e *EventListener) MasterLoop() {
// 	maybe, implement a dynamically growing event loop for multi threaded event handling
// }

// StartNewEventLoop creates and starts a new backing event loop to handle dispatched events.
// Silently ignored if the listener is stopped. Generates a new goroutine for the loop.
func (e *EventListener) StartNewEventLoop() {
	if e.stopped() {
		return
	}

	el := NewEventLoop()

	e.wg.Go(func() {
		el.loop(e.eventLoopChan)
	})

	e.eventLoops.AddLoop(el)
}

// fireEvent pushes the event and its handlers into the event loop channel if the listener is accepting events.
// Will block if the internal eventLoopChan buffer is full.
func (e *EventListener) fireEvent(event Event, handlers []EventHandler) {
	if !e.tryEnterFire() {
		return
	}
	defer e.leaveFire()

	e.eventLoopChan <- eventPair{event, handlers}
}

// stopped checks if the listener has been marked to stop accepting new events.
func (e *EventListener) stopped() bool {
	s := e.state.Load()
	return s&stoppedBit != 0
}

// stopAccepting sets the stoppedBit to prevent new events from being fired.
// Returns false if the listener was already stopped.
func (e *EventListener) stopAccepting() bool {
	for {
		s := e.state.Load()
		if s&stoppedBit != 0 {
			return false
		}
		if e.state.CompareAndSwap(s, s|stoppedBit) {
			ctx, cancel := context.WithCancel(context.Background())
			e.stopCtx.CompareAndSwap(nil, &ctxWrapper{ctx, cancel})
			return true
		}
	}
}

func (e *EventListener) stop() {
	e.eventLoops.Stop()
	e.wg.Wait()
	for {
		select {
		case ep := <-e.eventLoopChan:
			runEvent(ep.Event, ep.EventHandler)
		default:
			s := e.state.Load()
			active := s & countMask
			if active == 0 && len(e.eventLoopChan) == 0 {
				e.eventLoopChan.close()
				return
			}
			runtime.Gosched()
		}
	}
}

// Stop initiates a graceful shutdown of the event listener, preventing new events, stopping all event loops, and flushing pending events.
// Will block until all active fire operations complete and the channel buffers are completely drained.
func (e *EventListener) Stop() {
	ctx := e.StopAsync()
	<-ctx.Done()
}

func (e *EventListener) getStopCtxW() *ctxWrapper {
	ctxw := e.stopCtx.Load()
	for ctxw == nil {
		ctxw = e.stopCtx.Load()
	}
	return ctxw
}

// Stop initiates a graceful shutdown of the event listener, preventing new events, stopping all event loops, and flushing pending events.
// It runs in async. This means, that you won't know till it is actually done. But, the returned context can help you if you need to wait for it later.
func (e *EventListener) StopAsync() context.Context {
	if !e.stopAccepting() {
		return e.getStopCtxW().ctx
	}

	go func() {
		defer e.getStopCtxW().cancel()
		e.stop()
	}()

	return e.getStopCtxW().ctx
}
