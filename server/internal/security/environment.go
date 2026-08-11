package security

import (
	"bytes"
	"crypto/aes"
	"crypto/cipher"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"fmt"

	"golang.org/x/crypto/argon2"
)

const (
	EnvironmentKeyLength      = 32
	environmentKeySaltLength  = 16
	minimumSharedSecretLength = 32
	environmentValueMaxBytes  = 1024 * 1024
)

var (
	userKeySaltDomain = []byte("resolved/user-environment-key-salt/v1")
	valueDomain       = []byte("resolved/environment-variable-value/v1")
)

// EnvironmentCipher derives per-user encryption keys from the user's password
// and the deployment secret, then provides authenticated encryption for
// environment-variable values. Derived keys are never persisted.
type EnvironmentCipher struct {
	sharedSecret   []byte
	passwordParams PasswordParams
}

func NewEnvironmentCipher(sharedSecret string, passwordParams PasswordParams) (*EnvironmentCipher, error) {
	if len([]byte(sharedSecret)) < minimumSharedSecretLength {
		return nil, fmt.Errorf("environment encryption secret must contain at least %d bytes", minimumSharedSecretLength)
	}
	if passwordParams.Memory < 8*1024 || passwordParams.Memory > 1024*1024 ||
		passwordParams.Iterations == 0 || passwordParams.Iterations > 10 ||
		passwordParams.Parallelism == 0 || passwordParams.Parallelism > 16 {
		return nil, errors.New("environment key derivation parameters are invalid")
	}

	secret := append([]byte(nil), []byte(sharedSecret)...)
	cipher := &EnvironmentCipher{
		sharedSecret:   secret,
		passwordParams: passwordParams,
	}
	return cipher, nil
}

func (c *EnvironmentCipher) DeriveUserKey(userID, password string) ([]byte, error) {
	if userID == "" {
		return nil, errors.New("user ID is required for environment key derivation")
	}

	// The salt is deterministic but user-specific, so the same authenticated
	// password and deployment secret always derive the same key without storing
	// key material or a random key salt in the database.
	saltMAC := hmac.New(sha256.New, c.sharedSecret)
	saltMAC.Write(userKeySaltDomain)
	writeLengthPrefixed(saltMAC, []byte(userID))
	saltMaterial := saltMAC.Sum(nil)
	defer clear(saltMaterial)
	salt := saltMaterial[:environmentKeySaltLength]

	// Keyed preprocessing supplies Argon2id with input that depends on both the
	// password and the deployment secret without concatenation ambiguity.
	mac := hmac.New(sha256.New, c.sharedSecret)
	mac.Write([]byte("resolved/user-environment-key/v1"))
	writeLengthPrefixed(mac, []byte(userID))
	writeLengthPrefixed(mac, []byte(password))
	passwordMaterial := mac.Sum(nil)
	defer clear(passwordMaterial)

	return argon2.IDKey(
		passwordMaterial,
		salt,
		c.passwordParams.Iterations,
		c.passwordParams.Memory,
		c.passwordParams.Parallelism,
		EnvironmentKeyLength,
	), nil
}

func (c *EnvironmentCipher) EncryptValue(key []byte, userID, variableID, value string) ([]byte, error) {
	if len([]byte(value)) > environmentValueMaxBytes {
		return nil, fmt.Errorf("environment variable value exceeds %d bytes", environmentValueMaxBytes)
	}
	return seal(key, []byte(value), associatedData(valueDomain, userID, variableID))
}

func (c *EnvironmentCipher) DecryptValue(key []byte, userID, variableID string, encrypted []byte) (string, error) {
	plaintext, err := open(key, encrypted, associatedData(valueDomain, userID, variableID))
	if err != nil {
		return "", fmt.Errorf("decrypt environment variable value: %w", err)
	}
	defer clear(plaintext)
	return string(plaintext), nil
}

func seal(key, plaintext, additionalData []byte) ([]byte, error) {
	aead, err := newGCM(key)
	if err != nil {
		return nil, err
	}
	nonce := make([]byte, aead.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return nil, fmt.Errorf("generate encryption nonce: %w", err)
	}
	return aead.Seal(nonce, nonce, plaintext, additionalData), nil
}

func open(key, encrypted, additionalData []byte) ([]byte, error) {
	aead, err := newGCM(key)
	if err != nil {
		return nil, err
	}
	if len(encrypted) < aead.NonceSize()+aead.Overhead() {
		return nil, errors.New("encrypted value is truncated")
	}
	nonce := encrypted[:aead.NonceSize()]
	plaintext, err := aead.Open(nil, nonce, encrypted[aead.NonceSize():], additionalData)
	if err != nil {
		return nil, errors.New("encrypted value authentication failed")
	}
	return plaintext, nil
}

func newGCM(key []byte) (cipher.AEAD, error) {
	if len(key) != EnvironmentKeyLength {
		return nil, fmt.Errorf("encryption key must contain %d bytes", EnvironmentKeyLength)
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, fmt.Errorf("initialize AES: %w", err)
	}
	aead, err := cipher.NewGCM(block)
	if err != nil {
		return nil, fmt.Errorf("initialize AES-GCM: %w", err)
	}
	return aead, nil
}

func associatedData(domain []byte, values ...string) []byte {
	var result bytes.Buffer
	writeLengthPrefixed(&result, domain)
	for _, value := range values {
		writeLengthPrefixed(&result, []byte(value))
	}
	return result.Bytes()
}

type byteWriter interface {
	Write([]byte) (int, error)
}

func writeLengthPrefixed(writer byteWriter, value []byte) {
	var length [4]byte
	binary.BigEndian.PutUint32(length[:], uint32(len(value)))
	_, _ = writer.Write(length[:])
	_, _ = writer.Write(value)
}
