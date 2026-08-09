package security

import "testing"

func TestPasswordHashRoundTrip(t *testing.T) {
	hasher := NewPasswordHasher(PasswordParams{
		Memory:      8 * 1024,
		Iterations:  1,
		Parallelism: 1,
		SaltLength:  16,
		KeyLength:   32,
	})

	encoded, err := hasher.Hash("a sufficiently long password")
	if err != nil {
		t.Fatalf("hash password: %v", err)
	}

	valid, err := hasher.Verify(encoded, "a sufficiently long password")
	if err != nil {
		t.Fatalf("verify password: %v", err)
	}
	if !valid {
		t.Fatal("expected the password to verify")
	}

	valid, err = hasher.Verify(encoded, "a different long password")
	if err != nil {
		t.Fatalf("verify wrong password: %v", err)
	}
	if valid {
		t.Fatal("expected the wrong password to be rejected")
	}
}

func TestValidatePasswordBounds(t *testing.T) {
	if err := ValidatePassword("too short"); err == nil {
		t.Fatal("expected a short password to be rejected")
	}
	if err := ValidatePassword("twelve chars!"); err != nil {
		t.Fatalf("expected a 12-character password to be accepted: %v", err)
	}
}
