package counters

import (
	"sync"
	"testing"
)

func TestAtomicCounterWrapperRefreshesAfterRelease(t *testing.T) {
	counters := NewAtomicCounters()
	releasing := NewAtomicCounter("field", counters)
	stale := NewAtomicCounter("field", counters)

	releasing.Increase(1)
	stale.Get()
	releasing.Release()

	fresh := NewAtomicCounter("field", counters)
	fresh.Increase(1)
	stale.Increase(1)

	value, ok := counters.Get("field")
	if !ok {
		t.Fatal("expected released counter to be recreated")
	}
	if value != 2 {
		t.Fatalf("shared counter = %d, want 2", value)
	}
	if value := stale.Get(); value != 2 {
		t.Fatalf("stale wrapper counter = %d, want 2", value)
	}
}

func TestAtomicCounterWrapperCompareAndSwapRefreshesAfterRelease(t *testing.T) {
	counters := NewAtomicCounters()
	stale := NewAtomicCounter("field", counters)

	stale.Increase(1)
	counters.Release("field")
	counters.Increase("field", 1)

	if !stale.CompareAndSwap(1, 2) {
		t.Fatal("compare and swap failed after counter recreation")
	}

	value, ok := counters.Get("field")
	if !ok || value != 2 {
		t.Fatalf("shared counter = %d, %t; want 2, true", value, ok)
	}
}

func TestAtomicCounterWrapperStablePathDoesNotAllocate(t *testing.T) {
	counters := NewAtomicCounters()
	counter := NewAtomicCounter("field", counters)
	counter.Get()

	if allocations := testing.AllocsPerRun(1_000, func() {
		counter.Increase(1)
	}); allocations != 0 {
		t.Fatalf("stable counter allocations = %f, want 0", allocations)
	}
}

func TestAtomicCounterWrappersRemainUsableDuringRelease(t *testing.T) {
	const wrapperCount = 32
	const operationsPerWrapper = 1_000

	counters := NewAtomicCounters()
	wrappers := make([]AtomicCounter, wrapperCount)
	for i := range wrappers {
		wrappers[i] = NewAtomicCounter("field", counters)
		wrappers[i].Get()
	}

	var wg sync.WaitGroup
	wg.Add(wrapperCount + 1)
	for _, counter := range wrappers {
		go func() {
			defer wg.Done()
			for range operationsPerWrapper {
				counter.Increase(1)
			}
		}()
	}
	go func() {
		defer wg.Done()
		for range operationsPerWrapper {
			counters.Release("field")
		}
	}()
	wg.Wait()

	counters.Release("field")
	for _, counter := range wrappers {
		counter.Increase(1)
	}

	value, ok := counters.Get("field")
	if !ok || value != wrapperCount {
		t.Fatalf("shared counter = %d, %t; want %d, true", value, ok, wrapperCount)
	}
}
