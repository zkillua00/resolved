package security

import (
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"fmt"
	"sync"
	"time"

	"gorm.io/gorm"
	"gorm.io/gorm/clause"
)

const (
	DataKeyLength       = 32
	dataEnvelopeVersion = 1
	deploymentScopeID   = "deployment"
)

var ErrDataKeyUnavailable = errors.New("data encryption key is unavailable")

type DataEncryptionKey struct {
	ScopeType     string    `gorm:"size:32;primaryKey"`
	ScopeID       string    `gorm:"size:128;primaryKey"`
	Generation    uint32    `gorm:"primaryKey"`
	ProviderKeyID string    `gorm:"size:255;not null"`
	WrappedKey    []byte    `gorm:"not null"`
	Active        bool      `gorm:"not null;index"`
	CreatedAt     time.Time `gorm:"not null"`
}

func (DataEncryptionKey) TableName() string {
	return "data_encryption_keys"
}

type DataScope struct {
	Type string
	ID   string
}

func DeploymentDataScope() DataScope {
	return DataScope{Type: "deployment", ID: deploymentScopeID}
}

func WorkspaceDataScope(workspaceID string) DataScope {
	return DataScope{Type: "workspace", ID: workspaceID}
}

func (s DataScope) validate() error {
	if s.Type != "deployment" && s.Type != "workspace" {
		return fmt.Errorf("unsupported data encryption scope %q", s.Type)
	}
	if s.ID == "" {
		return errors.New("data encryption scope ID is required")
	}
	return nil
}

type KeyProvider interface {
	WrapKey(context.Context, []byte, []byte) (providerKeyID string, wrapped []byte, err error)
	UnwrapKey(context.Context, string, []byte, []byte) ([]byte, error)
}

type StaticKeyProvider struct {
	keyID string
	aead  cipher.AEAD
}

func NewStaticKeyProvider(keyID, encodedKey string) (*StaticKeyProvider, error) {
	key, err := base64.StdEncoding.DecodeString(encodedKey)
	if err != nil {
		return nil, errors.New("RESOLVED_DATA_ENCRYPTION_KEY must be base64 encoded")
	}
	defer clear(key)
	if len(key) != DataKeyLength {
		return nil, fmt.Errorf("RESOLVED_DATA_ENCRYPTION_KEY must decode to %d bytes", DataKeyLength)
	}
	if keyID == "" {
		return nil, errors.New("data encryption provider key ID is required")
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, fmt.Errorf("initialize data wrapping key: %w", err)
	}
	aead, err := cipher.NewGCM(block)
	if err != nil {
		return nil, fmt.Errorf("initialize data key wrapping: %w", err)
	}
	return &StaticKeyProvider{keyID: keyID, aead: aead}, nil
}

func (p *StaticKeyProvider) WrapKey(
	_ context.Context,
	plaintext, additionalData []byte,
) (string, []byte, error) {
	nonce := make([]byte, p.aead.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return "", nil, fmt.Errorf("generate data key wrapping nonce: %w", err)
	}
	return p.keyID, p.aead.Seal(nonce, nonce, plaintext, additionalData), nil
}

func (p *StaticKeyProvider) UnwrapKey(
	_ context.Context,
	providerKeyID string,
	wrapped, additionalData []byte,
) ([]byte, error) {
	if providerKeyID != p.keyID {
		return nil, fmt.Errorf("%w: provider key %q is not configured", ErrDataKeyUnavailable, providerKeyID)
	}
	if len(wrapped) < p.aead.NonceSize()+p.aead.Overhead() {
		return nil, fmt.Errorf("%w: wrapped key is truncated", ErrDataKeyUnavailable)
	}
	plaintext, err := p.aead.Open(nil, wrapped[:p.aead.NonceSize()], wrapped[p.aead.NonceSize():], additionalData)
	if err != nil {
		return nil, fmt.Errorf("%w: wrapped key authentication failed", ErrDataKeyUnavailable)
	}
	return plaintext, nil
}

type cachedDataKey struct {
	key       []byte
	expiresAt time.Time
}

type DataCipher struct {
	provider KeyProvider
	cacheTTL time.Duration

	mu    sync.Mutex
	cache map[string]cachedDataKey
}

func NewDataCipher(provider KeyProvider) (*DataCipher, error) {
	if provider == nil {
		return nil, errors.New("data encryption key provider is required")
	}
	return &DataCipher{
		provider: provider,
		cacheTTL: 5 * time.Minute,
		cache:    make(map[string]cachedDataKey),
	}, nil
}

func (c *DataCipher) Encrypt(
	ctx context.Context,
	db *gorm.DB,
	scope DataScope,
	domain, recordID string,
	plaintext []byte,
) ([]byte, error) {
	if domain == "" || recordID == "" {
		return nil, errors.New("data encryption domain and record ID are required")
	}
	record, key, err := c.activeKey(ctx, db, scope)
	if err != nil {
		return nil, err
	}
	defer clear(key)
	aead, err := dataAEAD(key)
	if err != nil {
		return nil, err
	}
	nonce := make([]byte, aead.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return nil, fmt.Errorf("generate data encryption nonce: %w", err)
	}
	header := make([]byte, 5+len(nonce))
	header[0] = dataEnvelopeVersion
	binary.BigEndian.PutUint32(header[1:5], record.Generation)
	copy(header[5:], nonce)
	aad := dataAdditionalData(scope, domain, recordID, record.Generation)
	return aead.Seal(header, nonce, plaintext, aad), nil
}

func (c *DataCipher) Decrypt(
	ctx context.Context,
	db *gorm.DB,
	scope DataScope,
	domain, recordID string,
	encrypted []byte,
) ([]byte, error) {
	if err := scope.validate(); err != nil {
		return nil, err
	}
	if domain == "" || recordID == "" {
		return nil, errors.New("data encryption domain and record ID are required")
	}
	if len(encrypted) < 5 {
		return nil, errors.New("encrypted data is truncated")
	}
	if encrypted[0] != dataEnvelopeVersion {
		return nil, fmt.Errorf("unsupported encrypted data version %d", encrypted[0])
	}
	generation := binary.BigEndian.Uint32(encrypted[1:5])
	key, err := c.keyByGeneration(ctx, db, scope, generation)
	if err != nil {
		return nil, err
	}
	defer clear(key)
	aead, err := dataAEAD(key)
	if err != nil {
		return nil, err
	}
	headerLength := 5 + aead.NonceSize()
	if len(encrypted) < headerLength+aead.Overhead() {
		return nil, errors.New("encrypted data is truncated")
	}
	nonce := encrypted[5:headerLength]
	plaintext, err := aead.Open(
		nil,
		nonce,
		encrypted[headerLength:],
		dataAdditionalData(scope, domain, recordID, generation),
	)
	if err != nil {
		return nil, errors.New("encrypted data authentication failed")
	}
	return plaintext, nil
}

func (c *DataCipher) LookupDigest(
	ctx context.Context,
	db *gorm.DB,
	scope DataScope,
	domain, normalizedValue string,
) (string, error) {
	if domain == "" {
		return "", errors.New("lookup digest domain is required")
	}
	if _, _, err := c.activeKey(ctx, db, scope); err != nil {
		return "", err
	}
	// Equality indexes remain stable across payload-key rotations. Rotating the
	// lookup key is a separate reindexing operation because changing it makes
	// existing deterministic lookups unreachable.
	key, err := c.keyByGeneration(ctx, db, scope, 1)
	if err != nil {
		return "", err
	}
	defer clear(key)
	mac := hmac.New(sha256.New, key)
	_, _ = mac.Write(encodedAdditionalData("resolved/blind-index/v1", scope.Type, scope.ID, domain))
	_, _ = mac.Write([]byte(normalizedValue))
	return fmt.Sprintf("%x", mac.Sum(nil)), nil
}

func (c *DataCipher) RotateScopeKey(
	ctx context.Context,
	db *gorm.DB,
	scope DataScope,
) (uint32, error) {
	if err := scope.validate(); err != nil {
		return 0, err
	}
	var generation uint32
	err := db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var current DataEncryptionKey
		err := tx.Where("scope_type = ? AND scope_id = ? AND active = ?", scope.Type, scope.ID, true).
			Order("generation DESC").First(&current).Error
		if errors.Is(err, gorm.ErrRecordNotFound) {
			current.Generation = 0
		} else if err != nil {
			return err
		}
		generation = current.Generation + 1
		generated := make([]byte, DataKeyLength)
		if _, err := rand.Read(generated); err != nil {
			return fmt.Errorf("generate rotated scoped data key: %w", err)
		}
		defer clear(generated)
		providerKeyID, wrapped, err := c.provider.WrapKey(
			ctx, generated, keyAdditionalData(scope, generation),
		)
		if err != nil {
			return fmt.Errorf("wrap rotated scoped data key: %w", err)
		}
		if current.Generation != 0 {
			if err := tx.Model(&DataEncryptionKey{}).
				Where("scope_type = ? AND scope_id = ? AND active = ?", scope.Type, scope.ID, true).
				Update("active", false).Error; err != nil {
				return err
			}
		}
		record := DataEncryptionKey{
			ScopeType: scope.Type, ScopeID: scope.ID, Generation: generation,
			ProviderKeyID: providerKeyID, WrappedKey: wrapped, Active: true,
		}
		if err := tx.Create(&record).Error; err != nil {
			return err
		}
		c.putCached(scope, generation, generated)
		return nil
	})
	if err != nil {
		return 0, fmt.Errorf("rotate scoped data key: %w", err)
	}
	return generation, nil
}

func (c *DataCipher) activeKey(
	ctx context.Context,
	db *gorm.DB,
	scope DataScope,
) (DataEncryptionKey, []byte, error) {
	if err := scope.validate(); err != nil {
		return DataEncryptionKey{}, nil, err
	}
	var record DataEncryptionKey
	err := db.WithContext(ctx).
		Where("scope_type = ? AND scope_id = ? AND active = ?", scope.Type, scope.ID, true).
		Order("generation DESC").First(&record).Error
	if errors.Is(err, gorm.ErrRecordNotFound) {
		generated := make([]byte, DataKeyLength)
		if _, err := rand.Read(generated); err != nil {
			return DataEncryptionKey{}, nil, fmt.Errorf("generate scoped data key: %w", err)
		}
		providerKeyID, wrapped, err := c.provider.WrapKey(ctx, generated, keyAdditionalData(scope, 1))
		if err != nil {
			clear(generated)
			return DataEncryptionKey{}, nil, fmt.Errorf("wrap scoped data key: %w", err)
		}
		record = DataEncryptionKey{
			ScopeType: scope.Type, ScopeID: scope.ID, Generation: 1,
			ProviderKeyID: providerKeyID, WrappedKey: wrapped, Active: true,
		}
		createErr := db.WithContext(ctx).Clauses(clause.OnConflict{DoNothing: true}).Create(&record).Error
		if createErr != nil {
			clear(generated)
			return DataEncryptionKey{}, nil, fmt.Errorf("store scoped data key: %w", createErr)
		}
		var stored DataEncryptionKey
		if err := db.WithContext(ctx).First(
			&stored,
			"scope_type = ? AND scope_id = ? AND generation = ?",
			scope.Type, scope.ID, uint32(1),
		).Error; err != nil {
			clear(generated)
			return DataEncryptionKey{}, nil, fmt.Errorf("reload scoped data key: %w", err)
		}
		if stored.ProviderKeyID == providerKeyID && string(stored.WrappedKey) == string(wrapped) {
			c.putCached(scope, 1, generated)
			return stored, generated, nil
		}
		clear(generated)
		record = stored
	} else if err != nil {
		return DataEncryptionKey{}, nil, fmt.Errorf("load active scoped data key: %w", err)
	}
	key, err := c.unwrap(ctx, scope, record)
	return record, key, err
}

func (c *DataCipher) keyByGeneration(
	ctx context.Context,
	db *gorm.DB,
	scope DataScope,
	generation uint32,
) ([]byte, error) {
	if key, ok := c.getCached(scope, generation); ok {
		return key, nil
	}
	var record DataEncryptionKey
	if err := db.WithContext(ctx).First(
		&record,
		"scope_type = ? AND scope_id = ? AND generation = ?",
		scope.Type, scope.ID, generation,
	).Error; err != nil {
		return nil, fmt.Errorf("%w: load scoped key generation %d: %v", ErrDataKeyUnavailable, generation, err)
	}
	return c.unwrap(ctx, scope, record)
}

func (c *DataCipher) unwrap(
	ctx context.Context,
	scope DataScope,
	record DataEncryptionKey,
) ([]byte, error) {
	if key, ok := c.getCached(scope, record.Generation); ok {
		return key, nil
	}
	key, err := c.provider.UnwrapKey(
		ctx,
		record.ProviderKeyID,
		record.WrappedKey,
		keyAdditionalData(scope, record.Generation),
	)
	if err != nil {
		return nil, fmt.Errorf("unwrap scoped data key: %w", err)
	}
	if len(key) != DataKeyLength {
		clear(key)
		return nil, fmt.Errorf("%w: unwrapped key has invalid length", ErrDataKeyUnavailable)
	}
	c.putCached(scope, record.Generation, key)
	return key, nil
}

func (c *DataCipher) getCached(scope DataScope, generation uint32) ([]byte, bool) {
	cacheKey := fmt.Sprintf("%s\x00%s\x00%d", scope.Type, scope.ID, generation)
	c.mu.Lock()
	defer c.mu.Unlock()
	entry, ok := c.cache[cacheKey]
	if !ok {
		return nil, false
	}
	if !entry.expiresAt.After(time.Now().UTC()) {
		clear(entry.key)
		delete(c.cache, cacheKey)
		return nil, false
	}
	return append([]byte(nil), entry.key...), true
}

func (c *DataCipher) putCached(scope DataScope, generation uint32, key []byte) {
	cacheKey := fmt.Sprintf("%s\x00%s\x00%d", scope.Type, scope.ID, generation)
	c.mu.Lock()
	defer c.mu.Unlock()
	if previous, ok := c.cache[cacheKey]; ok {
		clear(previous.key)
	}
	c.cache[cacheKey] = cachedDataKey{
		key:       append([]byte(nil), key...),
		expiresAt: time.Now().UTC().Add(c.cacheTTL),
	}
}

func (c *DataCipher) Close() {
	c.mu.Lock()
	defer c.mu.Unlock()
	for cacheKey, entry := range c.cache {
		clear(entry.key)
		delete(c.cache, cacheKey)
	}
}

func dataAEAD(key []byte) (cipher.AEAD, error) {
	if len(key) != DataKeyLength {
		return nil, errors.New("data encryption key has invalid length")
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, fmt.Errorf("initialize data encryption: %w", err)
	}
	aead, err := cipher.NewGCM(block)
	if err != nil {
		return nil, fmt.Errorf("initialize authenticated data encryption: %w", err)
	}
	return aead, nil
}

func keyAdditionalData(scope DataScope, generation uint32) []byte {
	return encodedAdditionalData(
		"resolved/scoped-data-key/v1",
		scope.Type,
		scope.ID,
		fmt.Sprintf("%d", generation),
	)
}

func dataAdditionalData(scope DataScope, domain, recordID string, generation uint32) []byte {
	return encodedAdditionalData(
		"resolved/server-data/v1",
		scope.Type,
		scope.ID,
		domain,
		recordID,
		fmt.Sprintf("%d", generation),
	)
}

func encodedAdditionalData(values ...string) []byte {
	hash := sha256.New()
	for _, value := range values {
		var length [4]byte
		binary.BigEndian.PutUint32(length[:], uint32(len(value)))
		_, _ = hash.Write(length[:])
		_, _ = hash.Write([]byte(value))
	}
	return hash.Sum(nil)
}
