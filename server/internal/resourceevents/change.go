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
)

type Resource string

const (
	ResourceUser                Resource = "user"
	ResourceRole                Resource = "role"
	ResourceWorkspace           Resource = "workspace"
	ResourceCollection          Resource = "collection"
	ResourceRequest             Resource = "request"
	ResourceEnvironment         Resource = "environment"
	ResourceEnvironmentVariable Resource = "environment_variable"
)

type Audience struct {
	Everyone       bool     `json:"-"`
	Owners         bool     `json:"-"`
	UserIDs        []string `json:"-"`
	RoleIDs        []string `json:"-"`
	PermissionKeys []string `json:"-"`
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
	change.Audience.UserIDs = cloneStrings(change.Audience.UserIDs)
	change.Audience.RoleIDs = cloneStrings(change.Audience.RoleIDs)
	change.Audience.PermissionKeys = cloneStrings(change.Audience.PermissionKeys)
	return change
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
