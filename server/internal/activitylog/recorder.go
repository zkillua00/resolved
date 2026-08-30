package activitylog

import (
	"context"
	"encoding/json"
	"log"
	"reflect"
	"time"

	"resolved-server/internal/resourceevents"

	"github.com/google/uuid"
)

type Recorder struct {
	repository *Repository
	downstream resourceevents.Emitter
}

func NewRecorder(repository *Repository, downstream resourceevents.Emitter) *Recorder {
	return &Recorder{repository: repository, downstream: downstream}
}

func (r *Recorder) FireEvent(name string, data any) {
	if name == resourceevents.EventName {
		if change, ok := data.(resourceevents.Change); ok {
			r.record(change)
		}
	}
	if r.downstream != nil {
		r.downstream.FireEvent(name, data)
	}
}

func (r *Recorder) record(change resourceevents.Change) {
	kind := logKind(change.Resource)
	if kind == "" {
		return
	}
	diffs := boundedDiffs(change.Diffs)
	diffsJSON, err := json.Marshal(diffs)
	if err != nil {
		log.Printf("marshal %s log diff: %v", kind, err)
		return
	}
	if len(diffsJSON) > MaxDiffJSONBytes {
		diffsJSON, _ = json.Marshal([]resourceevents.Diff{{
			Field: "diff", From: "[OMITTED]", To: "[DIFF EXCEEDED STORAGE LIMIT]",
		}})
	}
	createdAt := change.OccurredAt.UTC()
	if createdAt.IsZero() {
		createdAt = time.Now().UTC()
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := r.repository.Create(ctx, Entry{
		ID: uuid.NewString(), Kind: kind, Resource: string(change.Resource),
		Action: string(change.Action), ResourceID: change.ResourceID,
		WorkspaceID: change.WorkspaceID, CollectionID: change.CollectionID,
		ActorUserID: change.ActorUserID, TargetName: change.TargetName,
		DiffsJSON: diffsJSON, CreatedAt: createdAt,
	}); err != nil {
		log.Printf("record %s log entry: %v", kind, err)
	}
}

func logKind(resource resourceevents.Resource) string {
	switch resource {
	case resourceevents.ResourceWorkspace,
		resourceevents.ResourceCollection,
		resourceevents.ResourceRequest:
		return KindChange
	case resourceevents.ResourceUser, resourceevents.ResourceRole,
		resourceevents.ResourceRequestExecution, resourceevents.ResourceServerSettings:
		return KindAudit
	default:
		return ""
	}
}

func boundedDiffs(diffs []resourceevents.Diff) []resourceevents.Diff {
	originalLength := len(diffs)
	if len(diffs) > MaxDiffsPerEntry {
		diffs = diffs[:MaxDiffsPerEntry]
	}
	bounded := make([]resourceevents.Diff, 0, len(diffs)+1)
	for _, diff := range diffs {
		if reflect.DeepEqual(diff.From, diff.To) {
			continue
		}
		bounded = append(bounded, resourceevents.Diff{
			Field: diff.Field,
			From:  boundedValue(diff.From),
			To:    boundedValue(diff.To),
		})
	}
	if originalLength > MaxDiffsPerEntry {
		bounded = append(bounded, resourceevents.Diff{
			Field: "diff", From: nil, To: "[ADDITIONAL CHANGES OMITTED]",
		})
	}
	return bounded
}

func boundedValue(value any) any {
	encoded, err := json.Marshal(value)
	if err != nil {
		return "[UNSERIALIZABLE]"
	}
	if len(encoded) > MaxDiffValueBytes {
		return "[VALUE EXCEEDED STORAGE LIMIT]"
	}
	return value
}
