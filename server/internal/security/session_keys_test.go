package security

import (
	"bytes"
	"testing"
	"time"
)

func TestSessionEnvironmentKeysAreTokenAndUserScoped(t *testing.T) {
	store := NewSessionEnvironmentKeys()
	original := bytes.Repeat([]byte{0x42}, EnvironmentKeyLength)
	if err := store.Put("token-a", "user-a", "password-hash-a", original, time.Now().Add(time.Hour)); err != nil {
		t.Fatalf("put session key: %v", err)
	}
	original[0] ^= 0xff

	if _, ok := store.Get("token-a", "user-b", "password-hash-a"); ok {
		t.Fatal("another user obtained the session key")
	}
	key, ok := store.Get("token-a", "user-a", "password-hash-a")
	if !ok {
		t.Fatal("session key was not found")
	}
	if key[0] != 0x42 {
		t.Fatal("store retained the caller's mutable key slice")
	}
	key[0] ^= 0xff
	keyAgain, ok := store.Get("token-a", "user-a", "password-hash-a")
	if !ok || keyAgain[0] != 0x42 {
		t.Fatal("store exposed its mutable key slice")
	}
	if _, ok := store.Get("token-a", "user-a", "changed-password-hash"); ok {
		t.Fatal("changed credentials retained the session key")
	}
	if _, ok := store.Get("token-a", "user-a", "password-hash-a"); ok {
		t.Fatal("credential mismatch did not evict the session key")
	}

	store.Delete("token-a")
	if _, ok := store.Get("token-a", "user-a", "password-hash-a"); ok {
		t.Fatal("deleted session key is still available")
	}
}

func TestSessionEnvironmentKeysExpireAndDeleteByUser(t *testing.T) {
	store := NewSessionEnvironmentKeys()
	key := bytes.Repeat([]byte{0x24}, EnvironmentKeyLength)
	if err := store.Put("expired", "user-a", "password-hash-a", key, time.Now().Add(-time.Second)); err != nil {
		t.Fatalf("put expired key: %v", err)
	}
	if _, ok := store.Get("expired", "user-a", "password-hash-a"); ok {
		t.Fatal("expired key is still available")
	}
	if err := store.Put("token-a", "user-a", "password-hash-a", key, time.Now().Add(time.Hour)); err != nil {
		t.Fatalf("put first user key: %v", err)
	}
	if err := store.Put("token-b", "user-b", "password-hash-b", key, time.Now().Add(time.Hour)); err != nil {
		t.Fatalf("put second user key: %v", err)
	}

	store.DeleteUser("user-a")
	if _, ok := store.AnyForUser("user-a", "password-hash-a"); ok {
		t.Fatal("deleted user's key is still available")
	}
	if _, ok := store.Get("token-b", "user-b", "password-hash-b"); !ok {
		t.Fatal("deleting one user removed another user's key")
	}
}
