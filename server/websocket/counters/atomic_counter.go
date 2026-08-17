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
	counter        atomic.Pointer[atomicCounter] // fast access cache
	atomicCounters *atomicCounters
}

// loadCounter loads the counter from the atomicCounters map if it is not already loaded
// this is done to avoid the overhead of loading the counter from the map on every operation
func (a *atomicCounterWrapper) loadCounter() *atomicCounter {
	for {
		if counter := a.counter.Load(); counter != nil {
			if !counter.released.Load() {
				return counter
			}
			a.counter.CompareAndSwap(counter, nil)
			continue
		}

		counter := a.atomicCounters.loadCounter(a.field)
		if a.counter.CompareAndSwap(nil, counter) {
			return counter
		}
	}
}

func (a *atomicCounterWrapper) refresh(counter *atomicCounter) {
	a.counter.CompareAndSwap(counter, nil)
}

// Increase increases the counter by delta
func (a *atomicCounterWrapper) Increase(delta int64) int64 {
	for {
		counter := a.loadCounter()
		value := counter.value.Add(delta)
		if !counter.released.Load() {
			return value
		}
		a.refresh(counter)
	}
}

// CompareAndSwap compares the counter with old and swaps it with new if they are equal
func (a *atomicCounterWrapper) CompareAndSwap(old, new int64) bool {
	for {
		counter := a.loadCounter()
		swapped := counter.value.CompareAndSwap(old, new)
		if !counter.released.Load() {
			return swapped
		}
		a.refresh(counter)
	}
}

// Decrease decreases the counter by delta
func (a *atomicCounterWrapper) Decrease(delta int64) int64 {
	return a.Increase(-delta)
}

// Get returns the current value of the counter
func (a *atomicCounterWrapper) Get() int64 {
	for {
		counter := a.loadCounter()
		value := counter.value.Load()
		if !counter.released.Load() {
			return value
		}
		a.refresh(counter)
	}
}

// Release releases the counter
func (a *atomicCounterWrapper) Release() {
	a.atomicCounters.Release(a.field)
	a.counter.Store(nil)
}

func NewAtomicCounter(field string, counters AtomicCounters) AtomicCounter {
	return &atomicCounterWrapper{
		field:          field,
		atomicCounters: counters.(*atomicCounters),
	}
}
