package resourceevents

import (
	"strings"
	"time"

	"github.com/google/uuid"
)

const EventName = "resource.changed"

type Action string

const (
	ActionCreated Action = "created"
	ActionUpdated Action = "updated"
	ActionDeleted Action = "deleted"
	ActionExecuted Action = "executed"
)

type Resource string

const (
	ResourceUser                Resource = "user"
	ResourceRole                Resource = "role"
	ResourceWorkspace           Resource = "workspace"
	ResourceCollection          Resource = "collection"
	ResourceRequest             Resource = "request"
	ResourceSharedHistory       Resource = "shared_history"
	ResourceEnvironment         Resource = "environment"
	ResourceEnvironmentVariable Resource = "environment_variable"
	ResourceRequestExecution    Resource = "request_execution"
)

type Audience struct {
	Everyone       bool     `json:"-"`
	Owners         bool     `json:"-"`
	UserIDs        []string `json:"-"`
	RoleIDs        []string `json:"-"`
	PermissionKeys []string `json:"-"`
}

type Diff struct {
	Field string `json:"field"`
	From  any    `json:"from"`
	To    any    `json:"to"`
}

type Change struct {
	EventID       string    `json:"event_id"`
	Resource      Resource  `json:"resource"`
	Action        Action    `json:"action"`
	ResourceID    string    `json:"resource_id"`
	WorkspaceID   string    `json:"workspace_id,omitempty"`
	CollectionID  string    `json:"collection_id,omitempty"`
	EnvironmentID string    `json:"environment_id,omitempty"`
	OccurredAt    time.Time `json:"occurred_at"`
	Audience      Audience  `json:"-"`
	ActorUserID   string    `json:"-"`
	TargetName    string    `json:"-"`
	Diffs         []Diff    `json:"-"`
}

type Emitter interface {
	FireEvent(name string, data any)
}

func Emit(emitter Emitter, change Change) {
	if emitter == nil {
		return
	}
	if change.EventID == "" {
		change.EventID = uuid.NewString()
	}
	if change.OccurredAt.IsZero() {
		change.OccurredAt = time.Now().UTC()
	}
	emitter.FireEvent(EventName, cloneChange(change))
}

func cloneChange(change Change) Change {
	change.EventID = strings.Clone(change.EventID)
	change.Resource = Resource(strings.Clone(string(change.Resource)))
	change.Action = Action(strings.Clone(string(change.Action)))
	change.ResourceID = strings.Clone(change.ResourceID)
	change.WorkspaceID = strings.Clone(change.WorkspaceID)
	change.CollectionID = strings.Clone(change.CollectionID)
	change.EnvironmentID = strings.Clone(change.EnvironmentID)
	change.ActorUserID = strings.Clone(change.ActorUserID)
	change.TargetName = strings.Clone(change.TargetName)
	change.Diffs = cloneDiffs(change.Diffs)
	change.Audience.UserIDs = cloneStrings(change.Audience.UserIDs)
	change.Audience.RoleIDs = cloneStrings(change.Audience.RoleIDs)
	change.Audience.PermissionKeys = cloneStrings(change.Audience.PermissionKeys)
	return change
}

func cloneDiffs(diffs []Diff) []Diff {
	if diffs == nil {
		return nil
	}
	cloned := make([]Diff, len(diffs))
	for index, diff := range diffs {
		cloned[index] = Diff{
			Field: strings.Clone(diff.Field),
			From:  cloneValue(diff.From),
			To:    cloneValue(diff.To),
		}
	}
	return cloned
}

func cloneValue(value any) any {
	switch value := value.(type) {
	case string:
		return strings.Clone(value)
	case []string:
		return cloneStrings(value)
	case []any:
		cloned := make([]any, len(value))
		for index := range value {
			cloned[index] = cloneValue(value[index])
		}
		return cloned
	case map[string]any:
		cloned := make(map[string]any, len(value))
		for key, nested := range value {
			cloned[strings.Clone(key)] = cloneValue(nested)
		}
		return cloned
	default:
		return value
	}
}

func cloneStrings(values []string) []string {
	if values == nil {
		return nil
	}
	cloned := make([]string, len(values))
	for index, value := range values {
		cloned[index] = strings.Clone(value)
	}
	return cloned
}
