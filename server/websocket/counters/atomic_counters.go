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
	_map.SynchronizedMap[string, *atomic.Int64]
}

func (a *atomicCounters) Increase(field string, delta int64) {
	counter, _ := a.LoadOrStore(field, &atomic.Int64{})
	counter.Add(delta)
}
func (a *atomicCounters) CompareAndSwap(field string, old, new int64) bool {
	counter, _ := a.LoadOrStore(field, &atomic.Int64{})
	return counter.CompareAndSwap(old, new)
}
func (a *atomicCounters) Decrease(field string, delta int64) {
	counter, _ := a.LoadOrStore(field, &atomic.Int64{})
	counter.Add(-delta)
}
func (a *atomicCounters) Get(field string) (int64, bool) {
	counter, ok := a.Load(field)
	if !ok {
		return 0, false
	}
	return counter.Load(), true
}
func (a *atomicCounters) Release(field string) {
	a.Delete(field)
}

func NewAtomicCounters() AtomicCounters {
	return &atomicCounters{
		SynchronizedMap: _map.SynchronizedMap[string, *atomic.Int64]{},
	}
}
