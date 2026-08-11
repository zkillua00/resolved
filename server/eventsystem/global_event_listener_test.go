package eventsystem

import (
	"testing"
)

func TestGlobalEventListener(t *testing.T) {
	global := Global()
	if global == nil {
		t.Errorf("Expected global listener to not be nil")
	}

	// Since init() starts 3 loops, we don't necessarily want to call Stop() on it
	// because other tests might be running in parallel if this was part of a broader suite,
	// but for the sake of isolated testing of eventsystem package, we should just ensure it isn't nil.
}
