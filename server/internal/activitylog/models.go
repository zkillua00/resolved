package activitylog

import (
	"time"
)

const (
	KindChange = "change"
	KindAudit  = "audit"

	MaxWorkspaceEntries = 1000
	MaxAuditEntries     = 5000
	DefaultPageLimit    = 30
	MaxPageLimit        = 100
	MaxDiffsPerEntry    = 256
	MaxDiffValueBytes   = 16 * 1024
	MaxDiffJSONBytes    = 512 * 1024
)

type Entry struct {
	ID               string `gorm:"type:char(36);primaryKey"`
	Kind             string `gorm:"size:16;not null;index"`
	Resource         string `gorm:"size:32;not null;index"`
	Action           string `gorm:"size:32;not null"`
	ResourceID       string `gorm:"type:char(36);not null;index"`
	WorkspaceID      string `gorm:"type:char(36);not null;default:'';index"`
	CollectionID     string `gorm:"type:char(36);not null;default:'';index"`
	ActorUserID      string `gorm:"type:char(36);not null;default:'';index"`
	ActorEmail       string `gorm:"size:254;not null;default:''"`
	ActorDisplayName string `gorm:"size:120;not null;default:''"`
	TargetName       string `gorm:"size:256;not null;default:''"`
	DiffsJSON        []byte `gorm:"not null"`
	EncryptedPayload []byte
	CreatedAt        time.Time `gorm:"not null;index"`
}

func (Entry) TableName() string {
	return "activity_log_entries"
}

type DiffView struct {
	Field string `json:"field"`
	From  any    `json:"from"`
	To    any    `json:"to"`
}

type EntryView struct {
	ID               string     `json:"id"`
	Kind             string     `json:"kind"`
	Resource         string     `json:"resource"`
	Action           string     `json:"action"`
	ResourceID       string     `json:"resource_id"`
	WorkspaceID      string     `json:"workspace_id,omitempty"`
	CollectionID     string     `json:"collection_id,omitempty"`
	ActorUserID      string     `json:"actor_user_id,omitempty"`
	ActorEmail       string     `json:"actor_email,omitempty"`
	ActorDisplayName string     `json:"actor_display_name,omitempty"`
	TargetName       string     `json:"target_name"`
	Diffs            []DiffView `json:"diffs"`
	CreatedAt        time.Time  `json:"created_at"`
}

type PageView struct {
	Entries      []EntryView `json:"entries"`
	OlderCursor  *string     `json:"older_cursor,omitempty"`
	NewerCursor  *string     `json:"newer_cursor,omitempty"`
	HasMoreNewer bool        `json:"has_more_newer"`
}

type ListInput struct {
	Cursor string
	After  string
	Limit  int
}

type entryCursor struct {
	CreatedAt time.Time
	ID        string
}

type pageQuery struct {
	Before *entryCursor
	After  *entryCursor
	Limit  int
}

type entryPage struct {
	Entries []Entry
	HasMore bool
}
