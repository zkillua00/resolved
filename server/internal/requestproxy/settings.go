package requestproxy

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/url"
	"sort"
	"strings"
	"sync"
	"time"

	"resolved-server/internal/problem"
	"resolved-server/internal/security"

	"gorm.io/gorm"
)

const (
	SettingsRecordID    = "default"
	ModeLocal           = "local"
	ModeServer          = "server"
	MaxHostOverrides    = 256
	MaxTargetLength     = 512
	MaxAllowlistEntries = 1024
	MaxRequestURLLength = 16384
)

type SettingsRecord struct {
	ID                  string `gorm:"size:50;primaryKey"`
	Mode                string `gorm:"size:20;not null;default:local"`
	OverridesCiphertext []byte
	AllowlistCiphertext []byte
	CreatedAt           time.Time `gorm:"not null"`
	UpdatedAt           time.Time `gorm:"not null"`
}

func (SettingsRecord) TableName() string {
	return "request_execution_settings"
}

type HostnameOverrideRecord struct {
	Hostname  string    `gorm:"size:253;primaryKey"`
	Target    string    `gorm:"size:512;not null"`
	CreatedAt time.Time `gorm:"not null"`
	UpdatedAt time.Time `gorm:"not null"`
}

func (HostnameOverrideRecord) TableName() string {
	return "request_hostname_overrides"
}

type HostnameOverride struct {
	Hostname string `json:"hostname" validate:"required,max=253"`
	Target   string `json:"target" validate:"required,max=512"`
}

type hostnameOverrideTarget struct {
	Host   string
	Scheme string
}

type Settings struct {
	Mode                 string   `json:"mode"`
	AllowlistedRequests  []string `json:"allowlisted_requests"`
	AllowlistedAddresses []string `json:"allowlisted_addresses"`
}

type AllowlistEntry struct {
	Kind  string `json:"kind" validate:"required,oneof=request address"`
	Value string `json:"value" validate:"required,max=16384"`
}

type allowlistPayload struct {
	Requests  []string `json:"requests"`
	Addresses []string `json:"addresses"`
}

type Policy struct {
	CookieJar bool   `json:"cookie_jar"`
	Mode      string `json:"mode"`
}

type SettingsRepository struct {
	db          *gorm.DB
	dataCipher  *security.DataCipher
	allowlistMu sync.Mutex
}

func NewSettingsRepository(db *gorm.DB, dataCipher ...*security.DataCipher) *SettingsRepository {
	repository := &SettingsRepository{db: db}
	if len(dataCipher) > 0 {
		repository.dataCipher = dataCipher[0]
	}
	return repository
}

func (r *SettingsRepository) Get(ctx context.Context) (Settings, error) {
	settings := Settings{
		Mode:                ModeLocal,
		AllowlistedRequests: []string{}, AllowlistedAddresses: []string{},
	}
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var record SettingsRecord
		if err := tx.First(&record, "id = ?", SettingsRecordID).Error; err != nil {
			if errors.Is(err, gorm.ErrRecordNotFound) {
				return nil
			}
			return err
		}
		settings.Mode = record.Mode
		if len(record.AllowlistCiphertext) > 0 {
			if r.dataCipher == nil {
				return security.ErrDataKeyUnavailable
			}
			plaintext, err := r.dataCipher.Decrypt(
				ctx, tx, security.DeploymentDataScope(), "request_destination_allowlist", record.ID,
				record.AllowlistCiphertext,
			)
			if err != nil {
				return fmt.Errorf("decrypt request destination allowlist: %w", err)
			}
			defer clear(plaintext)
			var payload allowlistPayload
			if err := json.Unmarshal(plaintext, &payload); err != nil {
				return fmt.Errorf("decode request destination allowlist: %w", err)
			}
			settings.AllowlistedRequests = payload.Requests
			settings.AllowlistedAddresses = payload.Addresses
		}
		return nil
	})
	if err != nil {
		return Settings{}, problem.Wrap(err, "load request execution settings")
	}
	return settings, nil
}

// legacyOverrides reads the pre-proxy server-wide hostname overrides from the
// encrypted settings blob or, before encryption-at-rest, the plaintext rows.
func (r *SettingsRepository) legacyOverrides(ctx context.Context) ([]HostnameOverride, error) {
	var overrides []HostnameOverride
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var record SettingsRecord
		if err := tx.First(&record, "id = ?", SettingsRecordID).Error; err != nil {
			if errors.Is(err, gorm.ErrRecordNotFound) {
				return nil
			}
			return err
		}
		if len(record.OverridesCiphertext) > 0 {
			if r.dataCipher == nil {
				return security.ErrDataKeyUnavailable
			}
			plaintext, err := r.dataCipher.Decrypt(
				ctx, tx, security.DeploymentDataScope(), "request_hostname_overrides", record.ID,
				record.OverridesCiphertext,
			)
			if err != nil {
				return fmt.Errorf("decrypt request hostname overrides: %w", err)
			}
			defer clear(plaintext)
			return json.Unmarshal(plaintext, &overrides)
		}
		var records []HostnameOverrideRecord
		if err := tx.Order("hostname ASC").Find(&records).Error; err != nil {
			return err
		}
		for _, override := range records {
			overrides = append(overrides, HostnameOverride{
				Hostname: override.Hostname,
				Target:   override.Target,
			})
		}
		return nil
	})
	if err != nil {
		return nil, problem.Wrap(err, "load legacy hostname overrides")
	}
	return overrides, nil
}

func (r *SettingsRepository) clearLegacyOverrides(ctx context.Context) error {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := tx.Model(&SettingsRecord{}).Where("id = ?", SettingsRecordID).
			Update("overrides_ciphertext", nil).Error; err != nil {
			return err
		}
		return tx.Session(&gorm.Session{AllowGlobalUpdate: true}).Delete(&HostnameOverrideRecord{}).Error
	})
	if err != nil {
		return problem.Wrap(err, "clear legacy hostname overrides")
	}
	return nil
}

func (r *SettingsRepository) AddAllowlistEntry(ctx context.Context, entry AllowlistEntry) (Settings, error) {
	r.allowlistMu.Lock()
	defer r.allowlistMu.Unlock()
	settings, err := r.Get(ctx)
	if err != nil {
		return Settings{}, err
	}
	normalized, err := normalizeAllowlistEntry(entry)
	if err != nil {
		return Settings{}, err
	}
	switch normalized.Kind {
	case "request":
		if !containsString(settings.AllowlistedRequests, normalized.Value) {
			settings.AllowlistedRequests = append(settings.AllowlistedRequests, normalized.Value)
		}
	case "address":
		if !containsString(settings.AllowlistedAddresses, normalized.Value) {
			settings.AllowlistedAddresses = append(settings.AllowlistedAddresses, normalized.Value)
		}
	}
	if len(settings.AllowlistedRequests)+len(settings.AllowlistedAddresses) > MaxAllowlistEntries {
		return Settings{}, invalidField("value", fmt.Sprintf("the allowlist may contain at most %d entries", MaxAllowlistEntries))
	}
	sort.Strings(settings.AllowlistedRequests)
	sort.Strings(settings.AllowlistedAddresses)
	return r.saveAllowlist(ctx, settings)
}

func (r *SettingsRepository) saveAllowlist(ctx context.Context, settings Settings) (Settings, error) {
	if r.dataCipher == nil {
		return Settings{}, problem.Wrap(security.ErrDataKeyUnavailable, "save request destination allowlist")
	}
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		plaintext, err := json.Marshal(allowlistPayload{
			Requests: settings.AllowlistedRequests, Addresses: settings.AllowlistedAddresses,
		})
		if err != nil {
			return fmt.Errorf("encode request destination allowlist: %w", err)
		}
		defer clear(plaintext)
		ciphertext, err := r.dataCipher.Encrypt(
			ctx, tx, security.DeploymentDataScope(), "request_destination_allowlist", SettingsRecordID, plaintext,
		)
		if err != nil {
			return fmt.Errorf("encrypt request destination allowlist: %w", err)
		}
		return tx.Model(&SettingsRecord{}).Where("id = ?", SettingsRecordID).
			Update("allowlist_ciphertext", ciphertext).Error
	})
	if err != nil {
		return Settings{}, problem.Wrap(err, "save request destination allowlist")
	}
	return settings, nil
}

func normalizeAllowlistEntry(entry AllowlistEntry) (AllowlistEntry, error) {
	switch entry.Kind {
	case "request":
		target, err := url.Parse(strings.TrimSpace(entry.Value))
		if err != nil || target.Host == "" || target.User != nil || (target.Scheme != "http" && target.Scheme != "https") {
			return AllowlistEntry{}, invalidField("value", "must be an absolute HTTP or HTTPS URL without credentials")
		}
		target.Fragment = ""
		if len(target.String()) > MaxRequestURLLength {
			return AllowlistEntry{}, invalidField("value", fmt.Sprintf("must contain at most %d characters", MaxRequestURLLength))
		}
		return AllowlistEntry{Kind: entry.Kind, Value: target.String()}, nil
	case "address":
		address, valid := normalizeOverrideHost(entry.Value)
		if !valid {
			return AllowlistEntry{}, invalidField("value", "must be a hostname or IP address without a port")
		}
		return AllowlistEntry{Kind: entry.Kind, Value: address}, nil
	default:
		return AllowlistEntry{}, invalidField("kind", "must be request or address")
	}
}

func containsString(values []string, value string) bool {
	for _, candidate := range values {
		if candidate == value {
			return true
		}
	}
	return false
}

func (r *SettingsRepository) Replace(ctx context.Context, settings Settings) (Settings, error) {
	if settings.Mode != ModeLocal && settings.Mode != ModeServer {
		return Settings{}, invalidField("mode", "must be local or server")
	}
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var record SettingsRecord
		if err := tx.First(&record, "id = ?", SettingsRecordID).Error; err != nil {
			if !errors.Is(err, gorm.ErrRecordNotFound) {
				return err
			}
			record = SettingsRecord{ID: SettingsRecordID, Mode: settings.Mode}
			return tx.Create(&record).Error
		}
		return tx.Model(&record).Update("mode", settings.Mode).Error
	})
	if err != nil {
		return Settings{}, problem.Wrap(err, "save request execution settings")
	}
	return r.Get(ctx)
}

func normalizeDNSHostname(value string) (string, bool) {
	hostname := strings.ToLower(strings.TrimSuffix(strings.TrimSpace(value), "."))
	if hostname == "" || len(hostname) > 253 {
		return "", false
	}
	for _, label := range strings.Split(hostname, ".") {
		if len(label) == 0 || len(label) > 63 || !isASCIIAlphaNumeric(label[0]) || !isASCIIAlphaNumeric(label[len(label)-1]) {
			return "", false
		}
		for index := 1; index < len(label)-1; index++ {
			if !isASCIIAlphaNumeric(label[index]) && label[index] != '-' {
				return "", false
			}
		}
	}
	return hostname, true
}

func isASCIIAlphaNumeric(value byte) bool {
	return value >= 'a' && value <= 'z' || value >= '0' && value <= '9'
}

func parseHostnameOverrideTarget(value string) (hostnameOverrideTarget, bool) {
	value = strings.TrimSpace(value)
	if value == "" || len(value) > MaxTargetLength {
		return hostnameOverrideTarget{}, false
	}

	if strings.Contains(value, "://") {
		parsed, err := url.Parse(value)
		if err != nil || parsed.Host == "" || parsed.User != nil || parsed.Port() != "" || parsed.RawQuery != "" || parsed.Fragment != "" || parsed.Opaque != "" || parsed.Path != "" && parsed.Path != "/" {
			return hostnameOverrideTarget{}, false
		}
		scheme := strings.ToLower(parsed.Scheme)
		if scheme != "http" && scheme != "https" {
			return hostnameOverrideTarget{}, false
		}
		host, valid := normalizeOverrideHost(parsed.Hostname())
		if !valid {
			return hostnameOverrideTarget{}, false
		}
		return hostnameOverrideTarget{Host: host, Scheme: scheme}, true
	}

	host, valid := normalizeOverrideHost(value)
	if !valid {
		return hostnameOverrideTarget{}, false
	}
	return hostnameOverrideTarget{Host: host}, true
}

func normalizeOverrideHost(value string) (string, bool) {
	if ip := net.ParseIP(strings.Trim(value, "[]")); ip != nil {
		return ip.String(), true
	}
	return normalizeDNSHostname(value)
}

func (target hostnameOverrideTarget) String() string {
	host := target.Host
	if target.Scheme != "" && net.ParseIP(host) != nil && strings.Contains(host, ":") {
		host = "[" + host + "]"
	}
	if target.Scheme == "" {
		return host
	}
	return target.Scheme + "://" + host
}

func (settings Settings) allowlistMaps() (map[string]struct{}, map[string]struct{}) {
	requests := make(map[string]struct{}, len(settings.AllowlistedRequests))
	for _, value := range settings.AllowlistedRequests {
		requests[value] = struct{}{}
	}
	addresses := make(map[string]struct{}, len(settings.AllowlistedAddresses))
	for _, value := range settings.AllowlistedAddresses {
		addresses[normalizedHostname(value)] = struct{}{}
	}
	return requests, addresses
}
