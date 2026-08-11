package mutex

import (
	"flag"
	"log"
	"runtime/debug"
	"sync"
	"sync/atomic"

	"resolved-server/websocket/counters"
)

var mutexLog *bool = flag.Bool("mutexLog", false, "enables mutex lock/unlock logs")
var uid = atomic.Int64{}

var metrics = counters.NewAtomicCounters()

func getNextId() int64 {
	return uid.Add(1)
}

type RWMutexWrapper struct {
	id int64
	sync.RWMutex
}

func (r *RWMutexWrapper) log(format string, args ...any) {
	if *mutexLog {
		constructedArgs := []any{}
		for _, arg := range args {
			if v, ok := arg.(func() any); ok {
				constructedArgs = append(constructedArgs, v())
				continue
			}
			constructedArgs = append(constructedArgs, arg)
		}

		log.Printf(format, constructedArgs...)
	}
}

func stack() any {
	return string(debug.Stack())
}

func MakeRWMutex() RWMutexWrapper {
	return RWMutexWrapper{id: getNextId(), RWMutex: sync.RWMutex{}}
}

func (r *RWMutexWrapper) Lock() {
	r.log("[DEBUG] [RWMutex:%s] Lock called on: %s", r.id, stack)
	r.RWMutex.Lock()
}

func (r *RWMutexWrapper) Unlock() {
	r.log("[DEBUG] [RWMutex:%s] Unlock called on: %s", r.id, stack)
	r.RWMutex.Unlock()
}

func (r *RWMutexWrapper) RLock() {
	r.log("[DEBUG] [RWMutex:%s] RLock called on: %s", r.id, stack)
	r.RWMutex.RLock()
}

func (r *RWMutexWrapper) RUnlock() {
	r.log("[DEBUG] [RWMutex:%s] RUnlock called on: %s", r.id, stack)
	r.RWMutex.RUnlock()
}

func (r *RWMutexWrapper) TryLock() bool {
	r.log("[DEBUG] [RWMutex:%s] TryLock called on: %s", r.id, stack)
	return r.RWMutex.TryLock()
}

func (r *RWMutexWrapper) TryRLock() bool {
	r.log("[DEBUG] [RWMutex:%s] TryRLock called on: %s", r.id, stack)
	return r.RWMutex.TryRLock()
}
