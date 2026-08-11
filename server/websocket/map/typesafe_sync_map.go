package _map

import "sync"

type SynchronizedMap[K comparable, T any] struct {
	internal sync.Map
}

func (m *SynchronizedMap[K, T]) Store(key K, value T) {
	m.internal.Store(key, value)
}

func (m *SynchronizedMap[K, T]) Load(key K) (T, bool) {
	var zero T

	v, ok := m.internal.Load(key)
	if !ok {
		return zero, false
	}

	tv, okT := v.(T)
	if !okT {
		return zero, false
	}

	return tv, true
}

func (m *SynchronizedMap[K, T]) Delete(key K) {
	m.internal.Delete(key)
}

func (m *SynchronizedMap[K, T]) CompareAndSwap(key K, old, new T) bool {
	return m.internal.CompareAndSwap(key, old, new)
}

// CompareAndDelete deletes the entry for a key if its value is equal to old.
// (Go 1.20+)
func (m *SynchronizedMap[K, T]) CompareAndDelete(key K, old T) bool {
	return m.internal.CompareAndDelete(key, old)
}

// LoadOrStore returns the existing value for the key if present.
// Otherwise, it stores and returns the given value.
// The loaded result is true if the value was loaded, false if stored.
//
// Note: if the underlying map somehow has a non-T value for this key
// (e.g. via misuse), loaded will be false and the zero value of T is returned.
func (m *SynchronizedMap[K, T]) LoadOrStore(key K, value T) (T, bool) {
	var zero T

	v, loaded := m.internal.LoadOrStore(key, value)
	if !loaded {
		// We know we just stored `value` of type T.
		return value, false
	}

	tv, okT := v.(T)
	if !okT {
		return zero, false
	}

	return tv, true
}

// LoadAndDelete deletes the value for a key, returning the previous value if any.
// The loaded result reports whether the key was present.
func (m *SynchronizedMap[K, T]) LoadAndDelete(key K) (T, bool) {
	var zero T

	v, loaded := m.internal.LoadAndDelete(key)
	if !loaded {
		return zero, false
	}

	tv, okT := v.(T)
	if !okT {
		return zero, false
	}

	return tv, true
}

// Swap swaps the value for a key and returns the previous value if any.
// The loaded result reports whether the key was present. (Go 1.20+)
func (m *SynchronizedMap[K, T]) Swap(key K, value T) (T, bool) {
	var zero T

	v, loaded := m.internal.Swap(key, value)
	if !loaded {
		return zero, false
	}

	tv, okT := v.(T)
	if !okT {
		return zero, false
	}

	return tv, true
}

// Range calls f sequentially for each key and value present in the map.
// If f returns false, Range stops the iteration.
//
// Entries whose key or value are not of type K/T (should only happen if the
// map is misused outside this wrapper) are silently skipped.
func (m *SynchronizedMap[K, T]) Range(f func(key K, value T) bool) {
	m.internal.Range(func(k, v any) bool {
		kk, okK := k.(K)
		if !okK {
			return true
		}
		tv, okT := v.(T)
		if !okT {
			return true
		}
		return f(kk, tv)
	})
}

func (m *SynchronizedMap[K, T]) Len() int {
	var count int
	m.internal.Range(func(_, _ any) bool { count++; return true })
	return count
}

func (m *SynchronizedMap[K, T]) IsEmpty() bool {
	var hasItem = true
	m.internal.Range(func(_, _ any) bool { hasItem = false; return false })
	return !hasItem
}
