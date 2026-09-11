package requestproxy

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sort"

	"resolved-server/internal/identity"
	"resolved-server/internal/problem"
	"resolved-server/internal/security"
	"resolved-server/internal/workspaces"

	"github.com/google/uuid"
	"gorm.io/gorm"
)

const proxyPayloadKind = "request_proxy"

type ProxyRepository struct {
	db         *gorm.DB
	dataCipher *security.DataCipher
}

func NewProxyRepository(db *gorm.DB, dataCipher *security.DataCipher) *ProxyRepository {
	return &ProxyRepository{db: db, dataCipher: dataCipher}
}

func (r *ProxyRepository) List(ctx context.Context) ([]Proxy, error) {
	var proxies []Proxy
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var err error
		proxies, err = r.listProxies(ctx, tx)
		return err
	})
	if err != nil {
		return nil, problem.Wrap(err, "list proxies")
	}
	return proxies, nil
}

func (r *ProxyRepository) Get(ctx context.Context, id string) (Proxy, error) {
	var proxy Proxy
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var record ProxyRecord
		if err := tx.First(&record, "id = ?", id).Error; err != nil {
			return err
		}
		var err error
		proxy, err = r.hydrateProxy(ctx, tx, record)
		return err
	})
	if err != nil {
		return Proxy{}, mapProxyError(err, "load proxy")
	}
	return proxy, nil
}

func (r *ProxyRepository) Create(
	ctx context.Context,
	createdByUserID *string,
	name string,
	rules []HostnameOverride,
) (Proxy, error) {
	normalizedName, err := normalizeProxyName(name)
	if err != nil {
		return Proxy{}, err
	}
	normalizedRules, err := normalizeOverrideRules("rules", rules)
	if err != nil {
		return Proxy{}, err
	}
	record := ProxyRecord{ID: uuid.NewString(), CreatedByUserID: createdByUserID}
	err = r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var count int64
		if err := tx.Model(&ProxyRecord{}).Count(&count).Error; err != nil {
			return err
		}
		if count >= MaxProxies {
			return invalidField("name", fmt.Sprintf("the server may hold at most %d proxies", MaxProxies))
		}
		ciphertext, err := r.encryptPayload(ctx, tx, record.ID, proxyPayload{Name: normalizedName, Rules: normalizedRules})
		if err != nil {
			return err
		}
		record.PayloadCiphertext = ciphertext
		return tx.Create(&record).Error
	})
	if err != nil {
		return Proxy{}, mapProxyError(err, "create proxy")
	}
	return Proxy{
		ID:              record.ID,
		Name:            normalizedName,
		Rules:           normalizedRules,
		Assignments:     []ProxyAssignment{},
		ExcludedUserIDs: []string{},
		ExcludedRoleIDs: []string{},
		CreatedAt:       record.CreatedAt,
		UpdatedAt:       record.UpdatedAt,
	}, nil
}

func (r *ProxyRepository) Update(
	ctx context.Context,
	id string,
	name *string,
	rules *[]HostnameOverride,
) (Proxy, error) {
	var proxy Proxy
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var record ProxyRecord
		if err := tx.First(&record, "id = ?", id).Error; err != nil {
			return err
		}
		payload, err := r.decryptPayload(ctx, tx, record)
		if err != nil {
			return err
		}
		if name != nil {
			payload.Name, err = normalizeProxyName(*name)
			if err != nil {
				return err
			}
		}
		if rules != nil {
			payload.Rules, err = normalizeOverrideRules("rules", *rules)
			if err != nil {
				return err
			}
		}
		ciphertext, err := r.encryptPayload(ctx, tx, record.ID, payload)
		if err != nil {
			return err
		}
		if err := tx.Model(&record).Update("payload_ciphertext", ciphertext).Error; err != nil {
			return err
		}
		proxy, err = r.hydrateProxy(ctx, tx, record)
		proxy.Name = payload.Name
		proxy.Rules = payload.Rules
		return err
	})
	if err != nil {
		return Proxy{}, mapProxyError(err, "update proxy")
	}
	return proxy, nil
}

func (r *ProxyRepository) Delete(ctx context.Context, id string) (Proxy, error) {
	var proxy Proxy
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var record ProxyRecord
		if err := tx.First(&record, "id = ?", id).Error; err != nil {
			return err
		}
		var err error
		proxy, err = r.hydrateProxy(ctx, tx, record)
		if err != nil {
			return err
		}
		if err := tx.Where("proxy_id = ?", id).Delete(&ProxyAssignmentRecord{}).Error; err != nil {
			return err
		}
		if err := tx.Where("proxy_id = ?", id).Delete(&ProxyExclusionRecord{}).Error; err != nil {
			return err
		}
		return tx.Delete(&record).Error
	})
	if err != nil {
		return Proxy{}, mapProxyError(err, "delete proxy")
	}
	return proxy, nil
}

// ReplaceAssignments swaps the full assignment set of one proxy. Every scope
// must exist, and a scope node may carry at most one proxy across the whole
// deployment.
func (r *ProxyRepository) ReplaceAssignments(
	ctx context.Context,
	proxyID string,
	assignments []ProxyAssignment,
) (Proxy, error) {
	if len(assignments) > MaxProxyAssignments {
		return Proxy{}, invalidField(
			"assignments",
			fmt.Sprintf("must contain at most %d entries", MaxProxyAssignments),
		)
	}
	normalized, err := normalizeAssignments(assignments)
	if err != nil {
		return Proxy{}, err
	}
	var proxy Proxy
	err = r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var record ProxyRecord
		if err := tx.First(&record, "id = ?", proxyID).Error; err != nil {
			return err
		}
		for index, assignment := range normalized {
			if err := assertScopeExists(tx, index, assignment); err != nil {
				return err
			}
			var conflicting ProxyAssignmentRecord
			err := tx.Where(
				"scope_kind = ? AND scope_id = ? AND proxy_id <> ?",
				assignment.ScopeKind, assignment.ScopeID, proxyID,
			).First(&conflicting).Error
			if err == nil {
				return problem.New(
					problem.KindConflict,
					"proxy_scope_taken",
					fmt.Sprintf("the %s scope is already assigned to another proxy", assignment.ScopeKind),
				)
			}
			if !errors.Is(err, gorm.ErrRecordNotFound) {
				return err
			}
		}
		if err := tx.Where("proxy_id = ?", proxyID).Delete(&ProxyAssignmentRecord{}).Error; err != nil {
			return err
		}
		for _, assignment := range normalized {
			record := ProxyAssignmentRecord{
				ScopeKind: assignment.ScopeKind,
				ScopeID:   assignment.ScopeID,
				ProxyID:   proxyID,
			}
			if err := tx.Create(&record).Error; err != nil {
				return err
			}
		}
		var err error
		proxy, err = r.hydrateProxy(ctx, tx, record)
		return err
	})
	if err != nil {
		return Proxy{}, mapProxyError(err, "replace proxy assignments")
	}
	return proxy, nil
}

// ReplaceExclusions swaps the users and roles opted out of one proxy.
func (r *ProxyRepository) ReplaceExclusions(
	ctx context.Context,
	proxyID string,
	userIDs []string,
	roleIDs []string,
) (Proxy, error) {
	if len(userIDs)+len(roleIDs) > MaxProxyExclusions {
		return Proxy{}, invalidField(
			"excluded_user_ids",
			fmt.Sprintf("exclusions may contain at most %d entries", MaxProxyExclusions),
		)
	}
	userIDs = dedupeSortedIDs(userIDs)
	roleIDs = dedupeSortedIDs(roleIDs)
	var proxy Proxy
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var record ProxyRecord
		if err := tx.First(&record, "id = ?", proxyID).Error; err != nil {
			return err
		}
		if err := assertAllExist[identity.User](tx, "excluded_user_ids", userIDs); err != nil {
			return err
		}
		if err := assertAllExist[identity.Role](tx, "excluded_role_ids", roleIDs); err != nil {
			return err
		}
		if err := tx.Where("proxy_id = ?", proxyID).Delete(&ProxyExclusionRecord{}).Error; err != nil {
			return err
		}
		for _, userID := range userIDs {
			exclusion := ProxyExclusionRecord{ProxyID: proxyID, SubjectKind: ProxySubjectUser, SubjectID: userID}
			if err := tx.Create(&exclusion).Error; err != nil {
				return err
			}
		}
		for _, roleID := range roleIDs {
			exclusion := ProxyExclusionRecord{ProxyID: proxyID, SubjectKind: ProxySubjectRole, SubjectID: roleID}
			if err := tx.Create(&exclusion).Error; err != nil {
				return err
			}
		}
		var err error
		proxy, err = r.hydrateProxy(ctx, tx, record)
		return err
	})
	if err != nil {
		return Proxy{}, mapProxyError(err, "replace proxy exclusions")
	}
	return proxy, nil
}

// EffectiveOverrides resolves the hostname overrides that apply to one
// execution. Scopes are visited most specific first and the first proxy that
// covers a hostname wins. A proxy that excludes the acting user (directly or
// through one of their roles) is skipped entirely, so resolution falls
// through to the next scope.
func (r *ProxyRepository) EffectiveOverrides(
	ctx context.Context,
	scopes []ProxyScopeRef,
	userID string,
	roleIDs []string,
) (map[string]hostnameOverrideTarget, error) {
	if len(scopes) == 0 {
		return map[string]hostnameOverrideTarget{}, nil
	}
	overrides := make(map[string]hostnameOverrideTarget)
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		query := tx.Model(&ProxyAssignmentRecord{})
		condition := tx.Where("1 = 0")
		for _, scope := range scopes {
			condition = condition.Or(tx.Where("scope_kind = ? AND scope_id = ?", scope.Kind, scope.ID))
		}
		var assignments []ProxyAssignmentRecord
		if err := query.Where(condition).Find(&assignments).Error; err != nil {
			return err
		}
		if len(assignments) == 0 {
			return nil
		}
		assigned := make(map[ProxyScopeRef]string, len(assignments))
		proxyIDs := make([]string, 0, len(assignments))
		for _, assignment := range assignments {
			assigned[ProxyScopeRef{Kind: assignment.ScopeKind, ID: assignment.ScopeID}] = assignment.ProxyID
			proxyIDs = append(proxyIDs, assignment.ProxyID)
		}
		excluded, err := excludedProxyIDs(tx, proxyIDs, userID, roleIDs)
		if err != nil {
			return err
		}
		payloads := make(map[string]proxyPayload, len(proxyIDs))
		for _, scope := range scopes {
			proxyID, ok := assigned[scope]
			if !ok {
				continue
			}
			if _, skip := excluded[proxyID]; skip {
				continue
			}
			payload, loaded := payloads[proxyID]
			if !loaded {
				var record ProxyRecord
				if err := tx.First(&record, "id = ?", proxyID).Error; err != nil {
					if errors.Is(err, gorm.ErrRecordNotFound) {
						continue
					}
					return err
				}
				payload, err = r.decryptPayload(ctx, tx, record)
				if err != nil {
					return err
				}
				payloads[proxyID] = payload
			}
			for hostname, target := range overrideMapFromRules(payload.Rules) {
				if _, claimed := overrides[hostname]; !claimed {
					overrides[hostname] = target
				}
			}
		}
		return nil
	})
	if err != nil {
		return nil, problem.Wrap(err, "resolve effective hostname overrides")
	}
	return overrides, nil
}

// AdoptLegacyOverrides migrates the pre-proxy server-wide hostname overrides
// into a "Server default" proxy assigned server-wide, then clears the legacy
// storage. It runs once: when the legacy stores are empty it does nothing.
func (r *ProxyRepository) AdoptLegacyOverrides(ctx context.Context, settings *SettingsRepository) error {
	legacy, err := settings.legacyOverrides(ctx)
	if err != nil {
		return err
	}
	if len(legacy) == 0 {
		return nil
	}
	rules, err := normalizeOverrideRules("hostname_overrides", legacy)
	if err != nil {
		return err
	}
	err = r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		record := ProxyRecord{ID: uuid.NewString()}
		ciphertext, err := r.encryptPayload(ctx, tx, record.ID, proxyPayload{Name: "Server default", Rules: rules})
		if err != nil {
			return err
		}
		record.PayloadCiphertext = ciphertext
		if err := tx.Create(&record).Error; err != nil {
			return err
		}
		var conflicting ProxyAssignmentRecord
		err = tx.Where("scope_kind = ? AND scope_id = ?", ProxyScopeServer, "").First(&conflicting).Error
		if err == nil {
			// A proxy already owns the server-wide scope; keep it and only
			// preserve the legacy rules as an unassigned proxy.
			return nil
		}
		if !errors.Is(err, gorm.ErrRecordNotFound) {
			return err
		}
		assignment := ProxyAssignmentRecord{ScopeKind: ProxyScopeServer, ScopeID: "", ProxyID: record.ID}
		return tx.Create(&assignment).Error
	})
	if err != nil {
		return problem.Wrap(err, "adopt legacy hostname overrides")
	}
	return settings.clearLegacyOverrides(ctx)
}

func (r *ProxyRepository) listProxies(ctx context.Context, tx *gorm.DB) ([]Proxy, error) {
	var records []ProxyRecord
	if err := tx.Order("created_at ASC, id ASC").Find(&records).Error; err != nil {
		return nil, err
	}
	proxies := make([]Proxy, 0, len(records))
	for _, record := range records {
		proxy, err := r.hydrateProxy(ctx, tx, record)
		if err != nil {
			return nil, err
		}
		proxies = append(proxies, proxy)
	}
	return proxies, nil
}

func (r *ProxyRepository) hydrateProxy(ctx context.Context, tx *gorm.DB, record ProxyRecord) (Proxy, error) {
	payload, err := r.decryptPayload(ctx, tx, record)
	if err != nil {
		return Proxy{}, err
	}
	var assignmentRecords []ProxyAssignmentRecord
	if err := tx.Where("proxy_id = ?", record.ID).
		Order("scope_kind ASC, scope_id ASC").
		Find(&assignmentRecords).Error; err != nil {
		return Proxy{}, err
	}
	assignments := make([]ProxyAssignment, 0, len(assignmentRecords))
	for _, assignment := range assignmentRecords {
		assignments = append(assignments, ProxyAssignment{
			ScopeKind: assignment.ScopeKind,
			ScopeID:   assignment.ScopeID,
		})
	}
	var exclusionRecords []ProxyExclusionRecord
	if err := tx.Where("proxy_id = ?", record.ID).
		Order("subject_kind ASC, subject_id ASC").
		Find(&exclusionRecords).Error; err != nil {
		return Proxy{}, err
	}
	excludedUsers := []string{}
	excludedRoles := []string{}
	for _, exclusion := range exclusionRecords {
		switch exclusion.SubjectKind {
		case ProxySubjectUser:
			excludedUsers = append(excludedUsers, exclusion.SubjectID)
		case ProxySubjectRole:
			excludedRoles = append(excludedRoles, exclusion.SubjectID)
		}
	}
	return Proxy{
		ID:              record.ID,
		Name:            payload.Name,
		Rules:           payload.Rules,
		Assignments:     assignments,
		ExcludedUserIDs: excludedUsers,
		ExcludedRoleIDs: excludedRoles,
		CreatedAt:       record.CreatedAt,
		UpdatedAt:       record.UpdatedAt,
	}, nil
}

func (r *ProxyRepository) encryptPayload(
	ctx context.Context,
	tx *gorm.DB,
	proxyID string,
	payload proxyPayload,
) ([]byte, error) {
	if r.dataCipher == nil {
		return nil, security.ErrDataKeyUnavailable
	}
	if payload.Rules == nil {
		payload.Rules = []HostnameOverride{}
	}
	plaintext, err := json.Marshal(payload)
	if err != nil {
		return nil, fmt.Errorf("encode proxy payload: %w", err)
	}
	defer clear(plaintext)
	ciphertext, err := r.dataCipher.Encrypt(
		ctx, tx, security.DeploymentDataScope(), proxyPayloadKind, proxyID, plaintext,
	)
	if err != nil {
		return nil, fmt.Errorf("encrypt proxy payload: %w", err)
	}
	return ciphertext, nil
}

func (r *ProxyRepository) decryptPayload(
	ctx context.Context,
	tx *gorm.DB,
	record ProxyRecord,
) (proxyPayload, error) {
	if r.dataCipher == nil {
		return proxyPayload{}, security.ErrDataKeyUnavailable
	}
	plaintext, err := r.dataCipher.Decrypt(
		ctx, tx, security.DeploymentDataScope(), proxyPayloadKind, record.ID, record.PayloadCiphertext,
	)
	if err != nil {
		return proxyPayload{}, fmt.Errorf("decrypt proxy payload: %w", err)
	}
	defer clear(plaintext)
	var payload proxyPayload
	if err := json.Unmarshal(plaintext, &payload); err != nil {
		return proxyPayload{}, fmt.Errorf("decode proxy payload: %w", err)
	}
	if payload.Rules == nil {
		payload.Rules = []HostnameOverride{}
	}
	return payload, nil
}

func excludedProxyIDs(
	tx *gorm.DB,
	proxyIDs []string,
	userID string,
	roleIDs []string,
) (map[string]struct{}, error) {
	excluded := make(map[string]struct{})
	var exclusions []ProxyExclusionRecord
	query := tx.Where("proxy_id IN ?", proxyIDs)
	subjects := tx.Where("subject_kind = ? AND subject_id = ?", ProxySubjectUser, userID)
	if len(roleIDs) > 0 {
		subjects = subjects.Or(tx.Where("subject_kind = ? AND subject_id IN ?", ProxySubjectRole, roleIDs))
	}
	if err := query.Where(subjects).Find(&exclusions).Error; err != nil {
		return nil, err
	}
	for _, exclusion := range exclusions {
		excluded[exclusion.ProxyID] = struct{}{}
	}
	return excluded, nil
}

func normalizeAssignments(assignments []ProxyAssignment) ([]ProxyAssignment, error) {
	normalized := make([]ProxyAssignment, 0, len(assignments))
	seen := make(map[ProxyScopeRef]struct{}, len(assignments))
	for index, assignment := range assignments {
		switch assignment.ScopeKind {
		case ProxyScopeServer:
			if assignment.ScopeID != "" {
				return nil, invalidField(
					fmt.Sprintf("assignments.%d.scope_id", index),
					"must be empty for the server scope",
				)
			}
		case ProxyScopeWorkspace, ProxyScopeCollection, ProxyScopeRequest:
			if _, err := uuid.Parse(assignment.ScopeID); err != nil {
				return nil, invalidField(
					fmt.Sprintf("assignments.%d.scope_id", index),
					"must be a valid UUID",
				)
			}
		default:
			return nil, invalidField(
				fmt.Sprintf("assignments.%d.scope_kind", index),
				"must be server, workspace, collection, or request",
			)
		}
		key := ProxyScopeRef{Kind: assignment.ScopeKind, ID: assignment.ScopeID}
		if _, duplicate := seen[key]; duplicate {
			return nil, invalidField(
				fmt.Sprintf("assignments.%d", index),
				"duplicates another assignment",
			)
		}
		seen[key] = struct{}{}
		normalized = append(normalized, ProxyAssignment{
			ScopeKind: assignment.ScopeKind,
			ScopeID:   assignment.ScopeID,
		})
	}
	return normalized, nil
}

func assertScopeExists(tx *gorm.DB, index int, assignment ProxyAssignment) error {
	var count int64
	var err error
	switch assignment.ScopeKind {
	case ProxyScopeServer:
		return nil
	case ProxyScopeWorkspace:
		err = tx.Model(&workspaces.Workspace{}).Where("id = ?", assignment.ScopeID).Count(&count).Error
	case ProxyScopeCollection:
		err = tx.Model(&workspaces.Collection{}).Where("id = ?", assignment.ScopeID).Count(&count).Error
	case ProxyScopeRequest:
		err = tx.Model(&workspaces.SavedRequest{}).Where("id = ?", assignment.ScopeID).Count(&count).Error
	}
	if err != nil {
		return err
	}
	if count == 0 {
		return invalidField(
			fmt.Sprintf("assignments.%d.scope_id", index),
			fmt.Sprintf("references a %s that does not exist", assignment.ScopeKind),
		)
	}
	return nil
}

func assertAllExist[Model any](tx *gorm.DB, field string, ids []string) error {
	if len(ids) == 0 {
		return nil
	}
	for _, id := range ids {
		if _, err := uuid.Parse(id); err != nil {
			return invalidField(field, "must contain valid UUIDs")
		}
	}
	var count int64
	var model Model
	if err := tx.Model(&model).Where("id IN ?", ids).Count(&count).Error; err != nil {
		return err
	}
	if count != int64(len(ids)) {
		return invalidField(field, "references a subject that does not exist")
	}
	return nil
}

func dedupeSortedIDs(ids []string) []string {
	unique := make(map[string]struct{}, len(ids))
	for _, id := range ids {
		unique[id] = struct{}{}
	}
	result := make([]string, 0, len(unique))
	for id := range unique {
		result = append(result, id)
	}
	sort.Strings(result)
	return result
}

func mapProxyError(err error, operation string) error {
	if err == nil {
		return nil
	}
	var known *problem.Error
	if errors.As(err, &known) {
		return err
	}
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return problem.New(problem.KindNotFound, "proxy_not_found", "the proxy does not exist")
	}
	return problem.Wrap(err, operation)
}
