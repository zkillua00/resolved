package requestproxy

import (
	"context"
	"errors"
	"fmt"
	"net"
	"sort"
	"strings"
	"time"

	"resolved-server/internal/problem"

	"gorm.io/gorm"
)

const (
	SettingsRecordID = "default"
	ModeLocal        = "local"
	ModeServer       = "server"
	MaxHostOverrides = 256
)

type SettingsRecord struct {
	ID        string    `gorm:"size:50;primaryKey"`
	Mode      string    `gorm:"size:20;not null;default:local"`
	CreatedAt time.Time `gorm:"not null"`
	UpdatedAt time.Time `gorm:"not null"`
}

func (SettingsRecord) TableName() string {
	return "request_execution_settings"
}

type HostnameOverrideRecord struct {
	Hostname  string    `gorm:"size:253;primaryKey"`
	Target    string    `gorm:"size:253;not null"`
	CreatedAt time.Time `gorm:"not null"`
	UpdatedAt time.Time `gorm:"not null"`
}

func (HostnameOverrideRecord) TableName() string {
	return "request_hostname_overrides"
}

type HostnameOverride struct {
	Hostname string `json:"hostname" validate:"required,max=253"`
	Target   string `json:"target" validate:"required,max=253"`
}

type Settings struct {
	Mode              string             `json:"mode"`
	HostnameOverrides []HostnameOverride `json:"hostname_overrides"`
}

type Policy struct {
	Mode string `json:"mode"`
}

type SettingsRepository struct {
	db *gorm.DB
}

func NewSettingsRepository(db *gorm.DB) *SettingsRepository {
	return &SettingsRepository{db: db}
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
		var record SettingsRecord
		if err := tx.First(&record, "id = ?", SettingsRecordID).Error; err != nil {
			if !errors.Is(err, gorm.ErrRecordNotFound) {
				return err
			}
			record = SettingsRecord{ID: SettingsRecordID, Mode: normalized.Mode}
			if err := tx.Create(&record).Error; err != nil {
				return err
			}
		} else if err := tx.Model(&record).Update("mode", normalized.Mode).Error; err != nil {
			return err
		}
		if err := tx.Session(&gorm.Session{AllowGlobalUpdate: true}).Delete(&HostnameOverrideRecord{}).Error; err != nil {
			return err
		}
		if len(normalized.HostnameOverrides) == 0 {
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

		target := strings.TrimSpace(override.Target)
		if ip := net.ParseIP(strings.Trim(target, "[]")); ip != nil {
			target = ip.String()
		} else {
			var valid bool
			target, valid = normalizeDNSHostname(target)
			if !valid {
				return Settings{}, invalidField(
					fmt.Sprintf("hostname_overrides.%d.target", index),
					"must be a valid hostname or IP address without a scheme or port",
				)
			}
		}
		normalized.HostnameOverrides = append(normalized.HostnameOverrides, HostnameOverride{
			Hostname: hostname,
			Target:   target,
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

func (settings Settings) overrideMap() map[string]string {
	overrides := make(map[string]string, len(settings.HostnameOverrides))
	for _, override := range settings.HostnameOverrides {
		overrides[override.Hostname] = override.Target
	}
	return overrides
}
