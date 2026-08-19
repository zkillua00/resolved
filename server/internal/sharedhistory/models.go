package sharedhistory

import (
	"time"

	"resolved-server/internal/identity"
	"resolved-server/internal/workspaces"
)

const (
	MaxEntriesPerProfileWorkspace = 100
	MaxListedEntries              = 20
	MaxBodyBytes                  = 1024 * 1024
	MaxHeaders                    = 256
	MaxBodyFields                 = 256
	MaxHeaderBytes                = 512 * 1024
)

type Entry struct {
	ID                     string               `gorm:"type:char(36);primaryKey"`
	WorkspaceID            string               `gorm:"type:char(36);not null;index;uniqueIndex:idx_history_origin,priority:1"`
	Workspace              workspaces.Workspace `gorm:"foreignKey:WorkspaceID;references:ID;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	UserID                 string               `gorm:"type:char(36);not null;index;uniqueIndex:idx_history_origin,priority:2"`
	User                   identity.User        `gorm:"foreignKey:UserID;references:ID;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	ClientEntryID          string               `gorm:"size:128;not null;uniqueIndex:idx_history_origin,priority:3"`
	Method                 string               `gorm:"size:64;not null"`
	URL                    string               `gorm:"type:text;not null"`
	RequestHeadersJSON     []byte               `gorm:"not null"`
	RequestBody            []byte               `gorm:"not null"`
	RequestBodyMode        string               `gorm:"size:32;not null"`
	RequestBodyLanguage    string               `gorm:"size:32;not null"`
	RequestBodyFieldsJSON  []byte               `gorm:"not null"`
	RequestBodyTruncated   bool                 `gorm:"not null;default:false"`
	ResponseStatus         *int                 `gorm:"index"`
	ResponseStatusText     string               `gorm:"size:120;not null;default:''"`
	ResponseHTTPVersion    string               `gorm:"size:32;not null;default:''"`
	ResponseFinalURL       string               `gorm:"type:text;not null;default:''"`
	ResponseHeadersJSON    []byte               `gorm:"not null"`
	ResponseBody           []byte
	ResponseBodyTruncated  bool      `gorm:"not null;default:false"`
	ResponseContentType    string    `gorm:"size:512;not null;default:''"`
	ResponseDurationMicros *int64    `gorm:"index"`
	Error                  string    `gorm:"type:text;not null;default:''"`
	CreatedAt              time.Time `gorm:"not null;index"`
	UpdatedAt              time.Time `gorm:"not null"`
}

func (Entry) TableName() string {
	return "shared_history_entries"
}

type ProfileView struct {
	ID          string `json:"id"`
	Email       string `json:"email"`
	DisplayName string `json:"display_name"`
	Active      bool   `json:"active"`
}

type Header struct {
	Name  string `json:"name" validate:"required,max=4096"`
	Value string `json:"value" validate:"max=65536"`
}

type BodyField struct {
	Enabled bool   `json:"enabled"`
	Name    string `json:"name" validate:"max=4096"`
	Value   string `json:"value" validate:"max=1048576"`
	Kind    string `json:"kind" validate:"required,oneof=text file"`
}

type RequestView struct {
	Method        string      `json:"method"`
	URL           string      `json:"url"`
	Headers       []Header    `json:"headers"`
	Body          string      `json:"body"`
	BodyMode      string      `json:"body_mode"`
	BodyLanguage  string      `json:"raw_body_language"`
	BodyFields    []BodyField `json:"body_fields"`
	BodyTruncated bool        `json:"body_truncated"`
}

type ResponseView struct {
	Status         int      `json:"status"`
	StatusText     string   `json:"status_text"`
	HTTPVersion    string   `json:"http_version"`
	FinalURL       string   `json:"final_url"`
	Headers        []Header `json:"headers"`
	BodyBase64     string   `json:"body_base64"`
	BodyTruncated  bool     `json:"body_truncated"`
	ContentType    string   `json:"content_type,omitempty"`
	DurationMicros int64    `json:"duration_micros"`
}

type EntryView struct {
	ID        string        `json:"id"`
	CreatedAt time.Time     `json:"created_at"`
	Request   RequestView   `json:"request"`
	Response  *ResponseView `json:"response,omitempty"`
	Error     string        `json:"error,omitempty"`
}

type CreateInput struct {
	ClientEntryID string
	CreatedAt     time.Time
	Request       RequestView
	Response      *ResponseView
	Error         string
}
