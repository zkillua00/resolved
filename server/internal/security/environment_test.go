package security

import (
	"bytes"
	"testing"
)

func testEnvironmentPasswordParams() PasswordParams {
	return PasswordParams{
		Memory:      8 * 1024,
		Iterations:  1,
		Parallelism: 1,
		SaltLength:  16,
		KeyLength:   32,
	}
}

func TestEnvironmentKeyIsStableAndDependsOnPasswordSecretAndUser(t *testing.T) {
	first, err := NewEnvironmentCipher(
		"first deployment encryption secret is long enough",
		testEnvironmentPasswordParams(),
	)
	if err != nil {
		t.Fatalf("create first cipher: %v", err)
	}
	second, err := NewEnvironmentCipher(
		"second deployment encryption secret is also long enough",
		testEnvironmentPasswordParams(),
	)
	if err != nil {
		t.Fatalf("create second cipher: %v", err)
	}
	key, err := first.DeriveUserKey("user-a", "correct horse battery staple")
	if err != nil {
		t.Fatalf("derive environment key: %v", err)
	}
	same, err := first.DeriveUserKey("user-a", "correct horse battery staple")
	if err != nil {
		t.Fatalf("derive same environment key: %v", err)
	}
	if !bytes.Equal(key, same) {
		t.Fatal("same derivation inputs produced different keys")
	}

	tests := []struct {
		name   string
		cipher *EnvironmentCipher
		userID string
		pass   string
	}{
		{name: "password", cipher: first, userID: "user-a", pass: "different password"},
		{name: "secret", cipher: second, userID: "user-a", pass: "correct horse battery staple"},
		{name: "user", cipher: first, userID: "user-b", pass: "correct horse battery staple"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			other, err := test.cipher.DeriveUserKey(test.userID, test.pass)
			if err != nil {
				t.Fatalf("derive comparison key: %v", err)
			}
			if bytes.Equal(key, other) {
				t.Fatal("changed derivation input produced the same key")
			}
		})
	}
}

func TestEnvironmentValueEncryptionAuthenticatesItsOwnerAndVariable(t *testing.T) {
	cipher, err := NewEnvironmentCipher(
		"test deployment encryption secret with enough bytes",
		testEnvironmentPasswordParams(),
	)
	if err != nil {
		t.Fatalf("create environment cipher: %v", err)
	}
	key, err := cipher.DeriveUserKey("user-a", "correct horse battery staple")
	if err != nil {
		t.Fatalf("derive user key: %v", err)
	}
	plaintext := "a private API token"
	encrypted, err := cipher.EncryptValue(key, "user-a", "variable-a", plaintext)
	if err != nil {
		t.Fatalf("encrypt value: %v", err)
	}
	if bytes.Contains(encrypted, []byte(plaintext)) {
		t.Fatal("ciphertext contains the plaintext value")
	}
	decrypted, err := cipher.DecryptValue(key, "user-a", "variable-a", encrypted)
	if err != nil {
		t.Fatalf("decrypt value: %v", err)
	}
	if decrypted != plaintext {
		t.Fatalf("decrypted value = %q, want %q", decrypted, plaintext)
	}
	if _, err := cipher.DecryptValue(key, "user-b", "variable-a", encrypted); err == nil {
		t.Fatal("expected a different user to fail authentication")
	}
	if _, err := cipher.DecryptValue(key, "user-a", "variable-b", encrypted); err == nil {
		t.Fatal("expected a different variable to fail authentication")
	}

	tampered := append([]byte(nil), encrypted...)
	tampered[len(tampered)-1] ^= 0x01
	if _, err := cipher.DecryptValue(key, "user-a", "variable-a", tampered); err == nil {
		t.Fatal("expected tampered ciphertext to fail authentication")
	}
}

func TestEnvironmentCipherRejectsShortDeploymentSecret(t *testing.T) {
	if _, err := NewEnvironmentCipher("too short", testEnvironmentPasswordParams()); err == nil {
		t.Fatal("expected a short deployment secret to fail")
	}
}

func TestCookieJarEncryptionBindsPasswordUserWorkspaceAndPurpose(t *testing.T) {
	cipher, err := NewEnvironmentCipher("cookie jar deployment secret with enough bytes", testEnvironmentPasswordParams())
	if err != nil {
		t.Fatal(err)
	}
	key, _ := cipher.DeriveUserKey("alice", "original password")
	encrypted, err := cipher.EncryptCookieJar(key, "alice", "workspace-a", []byte("session=private"))
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(encrypted, []byte("private")) {
		t.Fatal("plaintext in ciphertext")
	}
	plaintext, err := cipher.DecryptCookieJar(key, "alice", "workspace-a", encrypted)
	if err != nil || string(plaintext) != "session=private" {
		t.Fatal("roundtrip failed")
	}
	wrongKey, _ := cipher.DeriveUserKey("alice", "new password")
	for _, test := range []struct {
		key             []byte
		user, workspace string
	}{{wrongKey, "alice", "workspace-a"}, {key, "bob", "workspace-a"}, {key, "alice", "workspace-b"}} {
		if _, err := cipher.DecryptCookieJar(test.key, test.user, test.workspace, encrypted); err == nil {
			t.Fatal("accepted mismatched binding")
		}
	}
	if _, err := cipher.DecryptValue(key, "alice", "workspace-a", encrypted); err == nil {
		t.Fatal("cookie ciphertext accepted as environment value")
	}
}
