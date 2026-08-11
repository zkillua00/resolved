package resourceevents

import (
	"testing"
	"unsafe"
)

type captureEmitter struct {
	change Change
}

func (e *captureEmitter) FireEvent(_ string, data any) {
	e.change = data.(Change)
}

func TestEmitOwnsStringsBeforeAsynchronousDispatch(t *testing.T) {
	buffer := []byte("workspace-1")
	borrowed := unsafe.String(unsafe.SliceData(buffer), len(buffer))
	emitter := &captureEmitter{}

	Emit(emitter, Change{
		Resource:    ResourceWorkspace,
		Action:      ActionUpdated,
		ResourceID:  borrowed,
		WorkspaceID: borrowed,
		Audience: Audience{
			UserIDs: []string{borrowed},
			RoleIDs: []string{borrowed},
		},
	})
	copy(buffer, "recycled-11")

	if emitter.change.ResourceID != "workspace-1" || emitter.change.WorkspaceID != "workspace-1" {
		t.Fatalf("event identifiers retained a borrowed request buffer: %+v", emitter.change)
	}
	if len(emitter.change.Audience.UserIDs) != 1 || emitter.change.Audience.UserIDs[0] != "workspace-1" {
		t.Fatalf("event audience retained a borrowed request buffer: %+v", emitter.change.Audience)
	}
	if len(emitter.change.Audience.RoleIDs) != 1 || emitter.change.Audience.RoleIDs[0] != "workspace-1" {
		t.Fatalf("event role audience retained a borrowed request buffer: %+v", emitter.change.Audience)
	}
}
