package eventsystem

import (
	"testing"
)

func TestEvent(t *testing.T) {
	e := NewEvent("test_event", "test_data")

	if Name(e) != "test_event" {
		t.Errorf("Expected name 'test_event', got '%s'", Name(e))
	}

	strData := Data[string](e)
	if !strData.HasValue() {
		t.Errorf("Expected value, got nil")
	}
	if strData.Unwrap() != "test_data" {
		t.Errorf("Expected data 'test_data', got '%s'", strData.Unwrap())
	}

	intData := Data[int](e)
	if intData.HasValue() {
		t.Errorf("Expected nil, got value")
	}
}
