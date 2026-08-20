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
	"time"

	"resolved-server/internal/problem"
	"resolved-server/internal/security"

	"gorm.io/gorm"
)

const (
	SettingsRecordID = "default"
	ModeLocal        = "local"
	ModeServer       = "server"
	MaxHostOverrides = 256
	MaxTargetLength  = 512
)

type SettingsRecord struct {
	ID                  string `gorm:"size:50;primaryKey"`
	Mode                string `gorm:"size:20;not null;default:local"`
	OverridesCiphertext []byte
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
	Mode              string             `json:"mode"`
	HostnameOverrides []HostnameOverride `json:"hostname_overrides"`
}

type Policy struct {
	Mode string `json:"mode"`
}

type SettingsRepository struct {
	db         *gorm.DB
	dataCipher *security.DataCipher
}

func NewSettingsRepository(db *gorm.DB, dataCipher ...*security.DataCipher) *SettingsRepository {
	repository := &SettingsRepository{db: db}
	if len(dataCipher) > 0 {
		repository.dataCipher = dataCipher[0]
	}
	return repository
}

func (r *SettingsRepository) Get(ctx context.Context) (Settings, error) {
	settings := Settings{Mode: ModeLocal, HostnameOverrides: []HostnameOverride{}}
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var record SettingsRecord
		if err := tx.First(&record, "id = ?", SettingsRecordID).Error; err != nil {
			if errors.Is(err, gorm.ErrRecordNotFound) {
				return nil
			}
			return err
		}
		settings.Mode = record.Mode
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
			if err := json.Unmarshal(plaintext, &settings.HostnameOverrides); err != nil {
				return fmt.Errorf("decode request hostname overrides: %w", err)
			}
			if settings.HostnameOverrides == nil {
				settings.HostnameOverrides = []HostnameOverride{}
			}
			return nil
		}

		var records []HostnameOverrideRecord
		if err := tx.Order("hostname ASC").Find(&records).Error; err != nil {
			return err
		}
		settings.HostnameOverrides = make([]HostnameOverride, 0, len(records))
		for _, override := range records {
			settings.HostnameOverrides = append(settings.HostnameOverrides, HostnameOverride{
				Hostname: override.Hostname,
				Target:   override.Target,
			})
		}
		return nil
	})
	if err != nil {
		return Settings{}, problem.Wrap(err, "load request execution settings")
	}
	return settings, nil
}

func (r *SettingsRepository) Replace(ctx context.Context, settings Settings) (Settings, error) {
	normalized, err := normalizeSettings(settings)
	if err != nil {
		return Settings{}, err
	}
	err = r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var overridesCiphertext []byte
		if r.dataCipher != nil {
			plaintext, marshalErr := json.Marshal(normalized.HostnameOverrides)
			if marshalErr != nil {
				return fmt.Errorf("encode request hostname overrides: %w", marshalErr)
			}
			defer clear(plaintext)
			overridesCiphertext, marshalErr = r.dataCipher.Encrypt(
				ctx, tx, security.DeploymentDataScope(), "request_hostname_overrides", SettingsRecordID, plaintext,
			)
			if marshalErr != nil {
				return fmt.Errorf("encrypt request hostname overrides: %w", marshalErr)
			}
		}
		var record SettingsRecord
		if err := tx.First(&record, "id = ?", SettingsRecordID).Error; err != nil {
			if !errors.Is(err, gorm.ErrRecordNotFound) {
				return err
			}
			record = SettingsRecord{
				ID: SettingsRecordID, Mode: normalized.Mode, OverridesCiphertext: overridesCiphertext,
			}
			if err := tx.Create(&record).Error; err != nil {
				return err
			}
		} else if err := tx.Model(&record).Updates(map[string]any{
			"mode": normalized.Mode, "overrides_ciphertext": overridesCiphertext,
		}).Error; err != nil {
			return err
		}
		if err := tx.Session(&gorm.Session{AllowGlobalUpdate: true}).Delete(&HostnameOverrideRecord{}).Error; err != nil {
			return err
		}
		if r.dataCipher != nil || len(normalized.HostnameOverrides) == 0 {
			return nil
		}
		records := make([]HostnameOverrideRecord, 0, len(normalized.HostnameOverrides))
		for _, override := range normalized.HostnameOverrides {
			records = append(records, HostnameOverrideRecord{
				Hostname: override.Hostname,
				Target:   override.Target,
			})
		}
		return tx.Create(&records).Error
	})
	if err != nil {
		return Settings{}, problem.Wrap(err, "save request execution settings")
	}
	return normalized, nil
}

func (r *SettingsRepository) EncryptLegacyOverrides(ctx context.Context) error {
	if r.dataCipher == nil {
		return security.ErrDataKeyUnavailable
	}
	var record SettingsRecord
	if err := r.db.WithContext(ctx).First(&record, "id = ?", SettingsRecordID).Error; err != nil {
		return err
	}
	if len(record.OverridesCiphertext) > 0 {
		return nil
	}
	var records []HostnameOverrideRecord
	if err := r.db.WithContext(ctx).Order("hostname ASC").Find(&records).Error; err != nil {
		return err
	}
	overrides := make([]HostnameOverride, 0, len(records))
	for _, override := range records {
		overrides = append(overrides, HostnameOverride{Hostname: override.Hostname, Target: override.Target})
	}
	_, err := r.Replace(ctx, Settings{Mode: record.Mode, HostnameOverrides: overrides})
	return err
}

func normalizeSettings(settings Settings) (Settings, error) {
	if settings.Mode != ModeLocal && settings.Mode != ModeServer {
		return Settings{}, invalidField("mode", "must be local or server")
	}
	if len(settings.HostnameOverrides) > MaxHostOverrides {
		return Settings{}, invalidField(
			"hostname_overrides",
			fmt.Sprintf("must contain at most %d entries", MaxHostOverrides),
		)
	}

	normalized := Settings{Mode: settings.Mode, HostnameOverrides: make([]HostnameOverride, 0, len(settings.HostnameOverrides))}
	seen := make(map[string]struct{}, len(settings.HostnameOverrides))
	for index, override := range settings.HostnameOverrides {
		hostname, ok := normalizeDNSHostname(override.Hostname)
		if !ok || net.ParseIP(hostname) != nil {
			return Settings{}, invalidField(
				fmt.Sprintf("hostname_overrides.%d.hostname", index),
				"must be a valid hostname, not an IP address",
			)
		}
		if _, duplicate := seen[hostname]; duplicate {
			return Settings{}, invalidField(
				fmt.Sprintf("hostname_overrides.%d.hostname", index),
				"duplicates another hostname override",
			)
		}
		seen[hostname] = struct{}{}

		target, valid := parseHostnameOverrideTarget(override.Target)
		if !valid {
			return Settings{}, invalidField(
				fmt.Sprintf("hostname_overrides.%d.target", index),
				"must be a hostname or IP, optionally prefixed with http:// or https://, without a port or path",
			)
		}
		normalized.HostnameOverrides = append(normalized.HostnameOverrides, HostnameOverride{
			Hostname: hostname,
			Target:   target.String(),
		})
	}
	sort.Slice(normalized.HostnameOverrides, func(left, right int) bool {
		return normalized.HostnameOverrides[left].Hostname < normalized.HostnameOverrides[right].Hostname
	})
	return normalized, nil
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

func (settings Settings) overrideMap() map[string]hostnameOverrideTarget {
	overrides := make(map[string]hostnameOverrideTarget, len(settings.HostnameOverrides))
	for _, override := range settings.HostnameOverrides {
		target, valid := parseHostnameOverrideTarget(override.Target)
		if valid {
			overrides[override.Hostname] = target
		}
	}
	return overrides
}
