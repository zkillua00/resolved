package requestproxy

import (
	"fmt"
	"net"
	"sort"
	"strings"
	"time"
)

// Proxies are named sets of hostname-override rules. A proxy only takes
// effect where it is assigned: server-wide, or to one workspace, collection,
// or saved request. Users and roles listed as exclusions are opted out of a
// proxy; for them resolution falls through to the next, less specific scope
// as if the excluded proxy did not exist.
const (
	MaxProxies          = 128
	MaxProxyNameLength  = 120
	MaxProxyAssignments = 256
	MaxProxyExclusions  = 256

	ProxyScopeServer     = "server"
	ProxyScopeWorkspace  = "workspace"
	ProxyScopeCollection = "collection"
	ProxyScopeRequest    = "request"

	ProxySubjectUser = "user"
	ProxySubjectRole = "role"
)

// ProxyRecord stores one proxy. The user-provided name and the hostname
// override rules are sensitive routing data and live in one encrypted payload,
// matching the deployment crypto model of the settings record.
type ProxyRecord struct {
	ID                string `gorm:"type:char(36);primaryKey"`
	PayloadCiphertext []byte
	CreatedByUserID   *string   `gorm:"type:char(36);index"`
	CreatedAt         time.Time `gorm:"not null"`
	UpdatedAt         time.Time `gorm:"not null"`
}

func (ProxyRecord) TableName() string {
	return "request_proxies"
}

// ProxyAssignmentRecord attaches a proxy to one scope node. The composite
// primary key enforces at most one proxy per node; the server-wide scope uses
// an empty ScopeID. Scope IDs are not foreign keys because they may reference
// a workspace, collection, or saved request; assignments whose scope has been
// deleted are inert during resolution.
type ProxyAssignmentRecord struct {
	ScopeKind string    `gorm:"size:20;primaryKey"`
	ScopeID   string    `gorm:"type:char(36);primaryKey"`
	ProxyID   string    `gorm:"type:char(36);not null;index"`
	CreatedAt time.Time `gorm:"not null"`
}

func (ProxyAssignmentRecord) TableName() string {
	return "request_proxy_assignments"
}

// ProxyExclusionRecord opts a user or role out of one proxy.
type ProxyExclusionRecord struct {
	ProxyID     string    `gorm:"type:char(36);primaryKey"`
	SubjectKind string    `gorm:"size:10;primaryKey"`
	SubjectID   string    `gorm:"type:char(36);primaryKey"`
	CreatedAt   time.Time `gorm:"not null"`
}

func (ProxyExclusionRecord) TableName() string {
	return "request_proxy_exclusions"
}

type Proxy struct {
	ID              string             `json:"id"`
	Name            string             `json:"name"`
	Rules           []HostnameOverride `json:"rules"`
	Assignments     []ProxyAssignment  `json:"assignments"`
	ExcludedUserIDs []string           `json:"excluded_user_ids"`
	ExcludedRoleIDs []string           `json:"excluded_role_ids"`
	CreatedAt       time.Time          `json:"created_at"`
	UpdatedAt       time.Time          `json:"updated_at"`
}

type ProxyAssignment struct {
	ScopeKind string `json:"scope_kind"`
	ScopeID   string `json:"scope_id,omitempty"`
}

// proxyPayload is the encrypted per-proxy content.
type proxyPayload struct {
	Name  string             `json:"name"`
	Rules []HostnameOverride `json:"rules"`
}

// ProxyScopeRef is one node of a resolution chain, ordered most specific
// first: request, then each collection ancestor from innermost to outermost,
// then the workspace, then the server-wide scope.
type ProxyScopeRef struct {
	Kind string
	ID   string
}

func normalizeProxyName(name string) (string, error) {
	trimmed := strings.TrimSpace(name)
	if trimmed == "" {
		return "", invalidField("name", "is required")
	}
	if len(trimmed) > MaxProxyNameLength {
		return "", invalidField("name", fmt.Sprintf("must contain at most %d characters", MaxProxyNameLength))
	}
	return trimmed, nil
}

// normalizeOverrideRules validates and canonicalizes a proxy's hostname
// override rules: lowercased unique hostnames, canonical targets, sorted by
// hostname.
func normalizeOverrideRules(field string, rules []HostnameOverride) ([]HostnameOverride, error) {
	if len(rules) > MaxHostOverrides {
		return nil, invalidField(field, fmt.Sprintf("must contain at most %d entries", MaxHostOverrides))
	}
	normalized := make([]HostnameOverride, 0, len(rules))
	seen := make(map[string]struct{}, len(rules))
	for index, rule := range rules {
		hostname, ok := normalizeDNSHostname(rule.Hostname)
		if !ok || net.ParseIP(hostname) != nil {
			return nil, invalidField(
				fmt.Sprintf("%s.%d.hostname", field, index),
				"must be a valid hostname, not an IP address",
			)
		}
		if _, duplicate := seen[hostname]; duplicate {
			return nil, invalidField(
				fmt.Sprintf("%s.%d.hostname", field, index),
				"duplicates another hostname override",
			)
		}
		seen[hostname] = struct{}{}

		target, valid := parseHostnameOverrideTarget(rule.Target)
		if !valid {
			return nil, invalidField(
				fmt.Sprintf("%s.%d.target", field, index),
				"must be a hostname or IP, optionally prefixed with http:// or https://, without a port or path",
			)
		}
		normalized = append(normalized, HostnameOverride{
			Hostname: hostname,
			Target:   target.String(),
		})
	}
	sort.Slice(normalized, func(left, right int) bool {
		return normalized[left].Hostname < normalized[right].Hostname
	})
	return normalized, nil
}

func overrideMapFromRules(rules []HostnameOverride) map[string]hostnameOverrideTarget {
	overrides := make(map[string]hostnameOverrideTarget, len(rules))
	for _, rule := range rules {
		target, valid := parseHostnameOverrideTarget(rule.Target)
		if valid {
			overrides[rule.Hostname] = target
		}
	}
	return overrides
}
