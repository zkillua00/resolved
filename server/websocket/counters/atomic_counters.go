package counters

import (
	"sync/atomic"

	_map "resolved-server/websocket/map"
)

type AtomicCounters interface {
	Increase(field string, delta int64)
	CompareAndSwap(field string, old, new int64) bool
	Decrease(field string, delta int64)
	Get(field string) (int64, bool)
	Release(field string)
}

type atomicCounters struct {
	_map.SynchronizedMap[string, *atomicCounter]
}

type atomicCounter struct {
	value    atomic.Int64
	released atomic.Bool
}

func (a *atomicCounters) loadCounter(field string) *atomicCounter {
	for {
		if counter, ok := a.Load(field); ok {
			if !counter.released.Load() {
				return counter
			}
			a.CompareAndDelete(field, counter)
			continue
		}

		counter, _ := a.LoadOrStore(field, &atomicCounter{})
		if !counter.released.Load() {
			return counter
		}
	}
}

func (a *atomicCounters) add(field string, delta int64) int64 {
	for {
		counter := a.loadCounter(field)
		value := counter.value.Add(delta)
		if !counter.released.Load() {
			return value
		}
	}
}

func (a *atomicCounters) compareAndSwap(field string, old, new int64) bool {
	for {
		counter := a.loadCounter(field)
		swapped := counter.value.CompareAndSwap(old, new)
		if !counter.released.Load() {
			return swapped
		}
	}
}

func (a *atomicCounters) Increase(field string, delta int64) {
	a.add(field, delta)
}
func (a *atomicCounters) CompareAndSwap(field string, old, new int64) bool {
	return a.compareAndSwap(field, old, new)
}
func (a *atomicCounters) Decrease(field string, delta int64) {
	a.add(field, -delta)
}
func (a *atomicCounters) Get(field string) (int64, bool) {
	for {
		counter, ok := a.Load(field)
		if !ok {
			return 0, false
		}
		if counter.released.Load() {
			a.CompareAndDelete(field, counter)
			continue
		}
		value := counter.value.Load()
		if !counter.released.Load() {
			return value, true
		}
	}
}
func (a *atomicCounters) Release(field string) {
	counter, ok := a.Load(field)
	if !ok {
		return
	}
	counter.released.Store(true)
	a.CompareAndDelete(field, counter)
}

func NewAtomicCounters() AtomicCounters {
	return &atomicCounters{
		SynchronizedMap: _map.SynchronizedMap[string, *atomicCounter]{},
	}
}
