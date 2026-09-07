package workspaces

import (
	"crypto/sha256"
	"gorm.io/driver/sqlite"
	"gorm.io/gorm"
	"path/filepath"
	"testing"
)

func TestCookieWritesRejectAnInFlightPreviousPasswordKey(t *testing.T) {
	db, err := gorm.Open(sqlite.Open(filepath.Join(t.TempDir(), "keys.sqlite")), &gorm.Config{})
	if err != nil {
		t.Fatal(err)
	}
	sqlDB, _ := db.DB()
	defer sqlDB.Close()
	if err := db.Exec("CREATE TABLE users (id TEXT PRIMARY KEY, password_hash TEXT, active BOOL)").Error; err != nil {
		t.Fatal(err)
	}
	db.Exec("INSERT INTO users VALUES (?, ?, ?)", "alice", "old-password-hash", true)
	actor := Actor{UserID: "alice", CredentialVersion: sha256.Sum256([]byte("old-password-hash"))}
	if err := requireCurrentCookieKey(db, actor); err != nil {
		t.Fatal(err)
	}
	db.Exec("UPDATE users SET password_hash = ? WHERE id = ?", "new-password-hash", "alice")
	if err := requireCurrentCookieKey(db, actor); err == nil {
		t.Fatal("accepted a request holding the previous password key")
	}
	actor.CredentialVersion = sha256.Sum256([]byte("new-password-hash"))
	if err := requireCurrentCookieKey(db, actor); err != nil {
		t.Fatal(err)
	}
	db.Exec("UPDATE users SET active = ? WHERE id = ?", false, "alice")
	if err := requireCurrentCookieKey(db, actor); err == nil {
		t.Fatal("accepted a deactivated user")
	}
}
