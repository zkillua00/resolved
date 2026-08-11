package eventsystem

import (
	"sync"

	"github.com/puzpuzpuz/xsync/v4"

	"resolved-server/eventsystem/maybe"
)

type Dependency any

// NamedDependencyMap is a thread-safe map of dependencies.
type NamedDependencyMap struct {
	m    *xsync.Map[string, Dependency]
	once sync.Once
}

// LoadMap is a thread-safe function that returns the map of dependencies.
// It uses a sync.Once to ensure that the map is initialized only once.
// It will block until the map is initialized. All users of the map will see the same map at the same time.
// if nmd is nil, it will return nil.
func LoadMap(nmd *NamedDependencyMap) *xsync.Map[string, Dependency] {
	if nmd == nil {
		return nil
	}

	nmd.once.Do(func() {
		nmd.m = xsync.NewMap[string, Dependency]()
	})
	return nmd.m
}

func (nmd *NamedDependencyMap) Inject(name string, dep Dependency) {
	if nmd == nil {
		return
	}
	LoadMap(nmd).Store(name, dep)
}

func ResolveDependency[T any](nmd *NamedDependencyMap, name string) maybe.Maybe[T] {
	if nmd == nil || nmd.m == nil {
		return maybe.Empty[T]()
	}

	dep, ok := nmd.m.Load(name)
	if !ok {
		return maybe.Empty[T]()
	}

	return maybe.Map(maybe.Some(dep), func(d Dependency) *T {
		if v, ok := d.(T); ok {
			return &v
		}
		return nil
	})
}
