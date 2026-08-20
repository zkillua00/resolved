package security

import (
	"bytes"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"testing"

	"github.com/google/uuid"
	"gorm.io/driver/sqlite"
	"gorm.io/gorm"
)

func TestDataCipherRoundTripTamperAndScopeBinding(t *testing.T) {
	db := openDataCipherTestDatabase(t)
	cipher := newTestDataCipher(t, db, bytes.Repeat([]byte{0x41}, DataKeyLength), "test-v1")
	defer cipher.Close()

	workspaceID := uuid.NewString()
	recordID := uuid.NewString()
	plaintext := []byte("authorization: bearer secret-value")
	encrypted, err := cipher.Encrypt(
		t.Context(), db, WorkspaceDataScope(workspaceID), "saved_request", recordID, plaintext,
	)
	if err != nil {
		t.Fatalf("encrypt data: %v", err)
	}
	if bytes.Contains(encrypted, plaintext) {
		t.Fatal("ciphertext contains plaintext")
	}
	decrypted, err := cipher.Decrypt(
		t.Context(), db, WorkspaceDataScope(workspaceID), "saved_request", recordID, encrypted,
	)
	if err != nil {
		t.Fatalf("decrypt data: %v", err)
	}
	if !bytes.Equal(decrypted, plaintext) {
		t.Fatalf("decrypted = %q, want %q", decrypted, plaintext)
	}
	clear(decrypted)

	for name, test := range map[string]struct {
		scope  DataScope
		domain string
		id     string
	}{
		"workspace": {scope: WorkspaceDataScope(uuid.NewString()), domain: "saved_request", id: recordID},
		"domain":    {scope: WorkspaceDataScope(workspaceID), domain: "shared_history", id: recordID},
		"record":    {scope: WorkspaceDataScope(workspaceID), domain: "saved_request", id: uuid.NewString()},
	} {
		t.Run(name, func(t *testing.T) {
			if _, err := cipher.Decrypt(t.Context(), db, test.scope, test.domain, test.id, encrypted); err == nil {
				t.Fatal("expected associated-data mismatch to fail")
			}
		})
	}

	tampered := append([]byte(nil), encrypted...)
	tampered[len(tampered)-1] ^= 0x01
	if _, err := cipher.Decrypt(
		t.Context(), db, WorkspaceDataScope(workspaceID), "saved_request", recordID, tampered,
	); err == nil {
		t.Fatal("expected tampered ciphertext to fail")
	}
}

func TestDataCipherStoresOnlyWrappedScopeKey(t *testing.T) {
	db := openDataCipherTestDatabase(t)
	root := bytes.Repeat([]byte{0x52}, DataKeyLength)
	cipher := newTestDataCipher(t, db, root, "root-v1")
	defer cipher.Close()

	scope := WorkspaceDataScope(uuid.NewString())
	if _, err := cipher.Encrypt(t.Context(), db, scope, "test", uuid.NewString(), []byte("secret")); err != nil {
		t.Fatalf("encrypt data: %v", err)
	}
	var stored DataEncryptionKey
	if err := db.First(&stored, "scope_type = ? AND scope_id = ?", scope.Type, scope.ID).Error; err != nil {
		t.Fatalf("load stored scoped key: %v", err)
	}
	if stored.ProviderKeyID != "root-v1" || len(stored.WrappedKey) <= DataKeyLength {
		t.Fatalf("stored key metadata = %+v", stored)
	}
	if bytes.Equal(stored.WrappedKey, root) {
		t.Fatal("database stored the root key")
	}

	wrongProvider, err := NewStaticKeyProvider(
		"root-v1",
		base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0x57}, DataKeyLength)),
	)
	if err != nil {
		t.Fatalf("create wrong provider: %v", err)
	}
	wrongCipher, err := NewDataCipher(wrongProvider)
	if err != nil {
		t.Fatalf("create wrong cipher: %v", err)
	}
	defer wrongCipher.Close()
	if _, err := wrongCipher.Decrypt(
		t.Context(), db, scope, "test", uuid.NewString(), []byte{dataEnvelopeVersion, 0, 0, 0, 1},
	); !errors.Is(err, ErrDataKeyUnavailable) {
		t.Fatalf("wrong provider error = %v, want ErrDataKeyUnavailable", err)
	}
}

func TestDataCipherScopeRotationKeepsOldCiphertextReadable(t *testing.T) {
	db := openDataCipherTestDatabase(t)
	cipher := newTestDataCipher(t, db, bytes.Repeat([]byte{0x61}, DataKeyLength), "root-v1")
	defer cipher.Close()
	scope := WorkspaceDataScope(uuid.NewString())
	firstID := uuid.NewString()
	first, err := cipher.Encrypt(t.Context(), db, scope, "request", firstID, []byte("first"))
	if err != nil {
		t.Fatalf("encrypt with first generation: %v", err)
	}
	generation, err := cipher.RotateScopeKey(t.Context(), db, scope)
	if err != nil {
		t.Fatalf("rotate scope key: %v", err)
	}
	if generation != 2 {
		t.Fatalf("rotated generation = %d, want 2", generation)
	}
	secondID := uuid.NewString()
	second, err := cipher.Encrypt(t.Context(), db, scope, "request", secondID, []byte("second"))
	if err != nil {
		t.Fatalf("encrypt with second generation: %v", err)
	}
	if got := binary.BigEndian.Uint32(first[1:5]); got != 1 {
		t.Fatalf("first ciphertext generation = %d", got)
	}
	if got := binary.BigEndian.Uint32(second[1:5]); got != 2 {
		t.Fatalf("second ciphertext generation = %d", got)
	}
	for _, test := range []struct {
		id         string
		ciphertext []byte
		want       string
	}{{firstID, first, "first"}, {secondID, second, "second"}} {
		plaintext, err := cipher.Decrypt(t.Context(), db, scope, "request", test.id, test.ciphertext)
		if err != nil {
			t.Fatalf("decrypt generation for %s: %v", test.want, err)
		}
		if string(plaintext) != test.want {
			t.Fatalf("decrypted = %q, want %q", plaintext, test.want)
		}
	}
}

func openDataCipherTestDatabase(t *testing.T) *gorm.DB {
	t.Helper()
	db, err := gorm.Open(sqlite.Open("file:"+uuid.NewString()+"?mode=memory&cache=shared"), &gorm.Config{})
	if err != nil {
		t.Fatalf("open database: %v", err)
	}
	sqlDB, err := db.DB()
	if err != nil {
		t.Fatalf("access database: %v", err)
	}
	t.Cleanup(func() { _ = sqlDB.Close() })
	if err := db.AutoMigrate(&DataEncryptionKey{}); err != nil {
		t.Fatalf("migrate data encryption keys: %v", err)
	}
	return db
}

func newTestDataCipher(t *testing.T, _ *gorm.DB, root []byte, keyID string) *DataCipher {
	t.Helper()
	provider, err := NewStaticKeyProvider(keyID, base64.StdEncoding.EncodeToString(root))
	if err != nil {
		t.Fatalf("create static key provider: %v", err)
	}
	cipher, err := NewDataCipher(provider)
	if err != nil {
		t.Fatalf("create data cipher: %v", err)
	}
	return cipher
}
