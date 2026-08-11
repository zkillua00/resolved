package counters

import "sync/atomic"

type AtomicCounter interface {
	Increase(delta int64) int64
	CompareAndSwap(old, new int64) bool
	Decrease(delta int64) int64
	Get() int64
	Release()
}

type atomicCounterWrapper struct {
	field          string
	counter        atomic.Pointer[atomic.Int64] // fast access cache
	atomicCounters *atomicCounters
}

// loadCounter loads the counter from the atomicCounters map if it is not already loaded
// this is done to avoid the overhead of loading the counter from the map on every operation
func (a *atomicCounterWrapper) loadCounter() *atomic.Int64 {
	if counter := a.counter.Load(); counter != nil {
		return counter
	}

	counter, _ := a.atomicCounters.LoadOrStore(a.field, &atomic.Int64{})
	if a.counter.CompareAndSwap(nil, counter) {
		return counter
	}

	return a.counter.Load()
}

// Increase increases the counter by delta
func (a *atomicCounterWrapper) Increase(delta int64) int64 {
	return a.loadCounter().Add(delta)
}

// CompareAndSwap compares the counter with old and swaps it with new if they are equal
func (a *atomicCounterWrapper) CompareAndSwap(old, new int64) bool {
	return a.loadCounter().CompareAndSwap(old, new)
}

// Decrease decreases the counter by delta
func (a *atomicCounterWrapper) Decrease(delta int64) int64 {
	return a.loadCounter().Add(-delta)
}

// Get returns the current value of the counter
func (a *atomicCounterWrapper) Get() int64 {
	return a.loadCounter().Load()
}

// Release releases the counter
func (a *atomicCounterWrapper) Release() {
	a.atomicCounters.Delete(a.field)
	a.counter.Store(nil)
}

func NewAtomicCounter(field string, counters AtomicCounters) AtomicCounter {
	return &atomicCounterWrapper{
		field:          field,
		atomicCounters: counters.(*atomicCounters),
	}
}
