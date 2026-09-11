// Package executionlimits owns live, inheritable execution policy.
package executionlimits

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"math"
	"strings"
	"time"

	"resolved-server/internal/problem"
	"resolved-server/internal/workspaces"

	"gorm.io/gorm"
)

type Bound struct {
	Unlimited bool  `json:"unlimited"`
	Value     int64 `json:"value"`
}
type Limits map[string]Bound
type Scope struct {
	WorkspaceID  string `json:"workspace_id,omitempty"`
	CollectionID string `json:"collection_id,omitempty"`
}
type Source struct {
	Kind         string `json:"kind"`
	WorkspaceID  string `json:"workspace_id,omitempty"`
	CollectionID string `json:"collection_id,omitempty"`
}
type Definition struct {
	Key     string `json:"key"`
	Label   string `json:"label"`
	Unit    string `json:"unit"`
	Default Bound  `json:"default"`
}
type Snapshot struct {
	Overrides   Limits            `json:"overrides"`
	Effective   Limits            `json:"effective"`
	Sources     map[string]Source `json:"sources"`
	Definitions []Definition      `json:"definitions"`
}

// Record stores only explicitly overridden keys. Empty IDs denote deployment.
type Record struct {
	WorkspaceID  string `gorm:"primaryKey;size:36"`
	CollectionID string `gorm:"primaryKey;size:36"`
	Key          string `gorm:"primaryKey;size:80"`
	Unlimited    bool   `gorm:"not null"`
	Value        int64  `gorm:"not null"`
}

func (Record) TableName() string { return "execution_limit_overrides" }

var definitions = []Definition{
	{"http.timeout_ms", "HTTP timeout", "ms", Bound{Value: 60000}},
	{"http.connect_timeout_ms", "Connect timeout", "ms", Bound{Value: 30000}},
	{"http.tls_handshake_timeout_ms", "TLS handshake timeout", "ms", Bound{Value: 10000}},
	{"http.request_bytes", "HTTP request body", "bytes", Bound{Value: 67108864}},
	{"http.response_bytes", "HTTP response body", "bytes", Bound{Value: 67108864}},
	{"http.envelope_bytes", "Execution envelope", "bytes", Bound{Value: 100663296}},
	{"http.redirects", "HTTP redirects", "count", Bound{Value: 10}},
	{"http.header_count", "Supplied HTTP request headers", "count", Bound{Value: 256}},
	{"http.url_bytes", "URL length", "bytes", Bound{Value: 16384}},
	{"websocket.handshake_timeout_ms", "WebSocket handshake timeout", "ms", Bound{Value: 60000}},
	{"websocket.opening_bytes", "WebSocket opening frame", "bytes", Bound{Value: 1048576}},
	{"websocket.message_bytes", "WebSocket message", "bytes", Bound{Value: 16777216}},
	{"websocket.concurrent_sessions", "Concurrent sessions per execution scope", "count", Bound{Value: 500}},
	{"websocket.script_source_bytes", "WebSocket combined script source", "bytes", Bound{Value: 1048576}},
	{"websocket.script_modules", "WebSocket imported modules", "count", Bound{Value: 64}},
	{"script.timeout_ms", "Script timeout", "ms", Bound{Value: 30000}},
	{"script.memory_bytes", "Script memory", "bytes", Bound{Value: 33554432}},
	{"script.stack_bytes", "Script stack", "bytes", Bound{Value: 262144}},
	{"script.source_bytes", "Script source", "bytes", Bound{Value: 262144}},
	{"script.body_bytes", "Script body", "bytes", Bound{Value: 5242880}},
	{"script.result_bytes", "Script result", "bytes", Bound{Value: 8388608}},
	{"script.log_entries", "Script log entries", "count", Bound{Value: 100}},
	{"script.log_bytes", "Script logs", "bytes", Bound{Value: 65536}},
	{"chain.max_depth", "Request chain depth", "count", Bound{Value: 16}},
	{"chain.max_requests", "Request chain requests", "count", Bound{Value: 64}},
}

func Validate(limits Limits) error {
	for key, bound := range limits {
		known := false
		for _, definition := range definitions {
			if definition.Key == key {
				known = true
				break
			}
		}
		message := ""
		switch {
		case !known:
			message = "unknown execution limit"
		case bound.Value < 0:
			message = "must not be negative"
		case bound.Unlimited && bound.Value != 0:
			message = "unlimited requires value zero"
		case strings.HasSuffix(key, "_ms") && bound.Value > math.MaxInt64/int64(time.Millisecond):
			message = "overflows duration conversion"
		case bound.Value > int64(int(^uint(0)>>1)):
			message = "overflows platform integer conversion"
		}
		if message != "" {
			return problem.WithFields("validation_failed", "invalid execution limits", map[string]string{key: message})
		}
	}
	return nil
}

type Provider struct{ db *gorm.DB }

func NewProvider(db *gorm.DB) *Provider { return &Provider{db: db} }

// Limitable lets each operation declare the subset it consumes without a
// central switch over operation types.
type Limitable interface {
	LimitScope() Scope
	LimitKeys() []string
}

func (p *Provider) Limit(ctx context.Context, target Limitable) (Limits, error) {
	snapshot, err := p.Resolve(ctx, target.LimitScope())
	if err != nil {
		return nil, err
	}
	result := Limits{}
	for _, key := range target.LimitKeys() {
		bound, ok := snapshot.Effective[key]
		if !ok {
			return nil, fmt.Errorf("unknown execution limit %q", key)
		}
		result[key] = bound
	}
	return result, nil
}

// Resolve reads parent links and every override within one database snapshot.
// No cache or fallback masks persistence errors.
func (p *Provider) Resolve(ctx context.Context, scope Scope) (Snapshot, error) {
	var result Snapshot
	err := p.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		layers, err := ancestry(tx, scope)
		if err != nil {
			return err
		}
		result, err = resolve(tx, layers)
		return err
	}, &sql.TxOptions{Isolation: sql.LevelRepeatableRead, ReadOnly: true})
	return result, err
}

func ancestry(tx *gorm.DB, scope Scope) ([]Scope, error) {
	layers := []Scope{{}}
	if scope.WorkspaceID == "" {
		if scope.CollectionID != "" {
			return nil, problem.WithFields("validation_failed", "invalid scope", map[string]string{"workspace_id": "required with collection_id"})
		}
		return layers, nil
	}
	var workspace workspaces.Workspace
	if err := tx.Select("id").First(&workspace, "id = ?", scope.WorkspaceID).Error; err != nil {
		return nil, scopeError(err)
	}
	layers = append(layers, Scope{WorkspaceID: scope.WorkspaceID})
	var reversed []Scope
	seen := map[string]bool{}
	for id := scope.CollectionID; id != ""; {
		if seen[id] {
			return nil, fmt.Errorf("execution limit collection ancestry cycle")
		}
		seen[id] = true
		var collection workspaces.Collection
		if err := tx.Select("id", "workspace_id", "parent_collection_id").First(&collection, "id = ? AND workspace_id = ?", id, scope.WorkspaceID).Error; err != nil {
			return nil, scopeError(err)
		}
		reversed = append(reversed, Scope{WorkspaceID: scope.WorkspaceID, CollectionID: id})
		id = ""
		if collection.ParentCollectionID != nil {
			id = *collection.ParentCollectionID
		}
	}
	for i := len(reversed) - 1; i >= 0; i-- {
		layers = append(layers, reversed[i])
	}
	return layers, nil
}
func scopeError(err error) error {
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return problem.New(problem.KindNotFound, "scope_not_found", "execution limit scope was not found")
	}
	return err
}
func resolve(tx *gorm.DB, layers []Scope) (Snapshot, error) {
	snapshot := Snapshot{Overrides: Limits{}, Effective: Limits{}, Sources: map[string]Source{}, Definitions: append([]Definition(nil), definitions...)}
	for _, definition := range definitions {
		snapshot.Effective[definition.Key] = definition.Default
		snapshot.Sources[definition.Key] = Source{Kind: "default"}
	}
	for i, layer := range layers {
		var records []Record
		if err := tx.Where("workspace_id = ? AND collection_id = ?", layer.WorkspaceID, layer.CollectionID).Find(&records).Error; err != nil {
			return Snapshot{}, err
		}
		values := Limits{}
		for _, record := range records {
			values[record.Key] = Bound{Unlimited: record.Unlimited, Value: record.Value}
		}
		if err := Validate(values); err != nil {
			return Snapshot{}, fmt.Errorf("invalid persisted execution limits: %w", err)
		}
		source := Source{Kind: "deployment", WorkspaceID: layer.WorkspaceID, CollectionID: layer.CollectionID}
		if layer.WorkspaceID != "" {
			source.Kind = "workspace"
		}
		if layer.CollectionID != "" {
			source.Kind = "collection"
		}
		for key, bound := range values {
			snapshot.Effective[key] = bound
			snapshot.Sources[key] = source
		}
		if i == len(layers)-1 {
			snapshot.Overrides = values
		}
	}
	return snapshot, nil
}

// Replace atomically replaces one layer; omitted keys inherit again.
func (p *Provider) Replace(ctx context.Context, scope Scope, limits Limits) (Snapshot, error) {
	result, _, err := p.replace(ctx, scope, limits, nil)
	return result, err
}
func (p *Provider) replace(ctx context.Context, scope Scope, limits Limits, authorize func(*gorm.DB, []Scope) error) (Snapshot, Limits, error) {
	if err := Validate(limits); err != nil {
		return Snapshot{}, nil, err
	}
	var result Snapshot
	var before Limits
	err := p.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		layers, err := ancestry(tx, scope)
		if err != nil {
			return err
		}
		if authorize != nil {
			if err := authorize(tx, layers); err != nil {
				return err
			}
		}
		previous, err := resolve(tx, layers)
		if err != nil {
			return err
		}
		before = previous.Overrides
		if err := tx.Where("workspace_id = ? AND collection_id = ?", scope.WorkspaceID, scope.CollectionID).Delete(&Record{}).Error; err != nil {
			return err
		}
		for key, bound := range limits {
			if err := tx.Create(&Record{WorkspaceID: scope.WorkspaceID, CollectionID: scope.CollectionID, Key: key, Unlimited: bound.Unlimited, Value: bound.Value}).Error; err != nil {
				return err
			}
		}
		result, err = resolve(tx, layers)
		return err
	}, &sql.TxOptions{Isolation: sql.LevelSerializable})
	return result, before, err
}
