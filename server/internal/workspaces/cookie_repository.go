package workspaces

import (
	"context"
	"encoding/json"
	"errors"
	"gorm.io/gorm"
	"gorm.io/gorm/clause"
	"resolved-server/internal/problem"
	"resolved-server/internal/security"
)

type cookieJarRepository struct {
	core   *Repository
	cipher *security.EnvironmentCipher
}

func (r *cookieJarRepository) Get(ctx context.Context, actor Actor, workspaceID string) (CookieJar, error) {
	return r.load(r.core.db.WithContext(ctx), actor, workspaceID)
}
func (r *cookieJarRepository) load(db *gorm.DB, actor Actor, workspaceID string) (CookieJar, error) {
	if err := requireCurrentCookieKey(db, actor); err != nil {
		return CookieJar{}, err
	}
	if len(actor.EnvironmentKey) != security.EnvironmentKeyLength {
		return CookieJar{}, security.ErrEnvironmentKeyUnavailable
	}
	var row CookieJarRecord
	err := db.Where("workspace_id = ? AND user_id = ?", workspaceID, actor.UserID).First(&row).Error
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return CookieJar{Enabled: true, Cookies: []JarCookie{}}, nil
	}
	if err != nil {
		return CookieJar{}, err
	}
	bytes, err := r.cipher.DecryptCookieJar(actor.EnvironmentKey, actor.UserID, workspaceID, row.Ciphertext)
	if err != nil {
		return CookieJar{}, problem.Wrap(err, "decrypt cookie jar")
	}
	defer clear(bytes)
	var jar CookieJar
	if err := json.Unmarshal(bytes, &jar); err != nil {
		return CookieJar{}, problem.Wrap(err, "decode cookie jar")
	}
	jar.Revision = row.Revision
	return jar, nil
}
func (r *cookieJarRepository) save(tx *gorm.DB, actor Actor, workspaceID string, jar CookieJar) error {
	bytes, err := json.Marshal(jar)
	if err != nil {
		return err
	}
	defer clear(bytes)
	ciphertext, err := r.cipher.EncryptCookieJar(actor.EnvironmentKey, actor.UserID, workspaceID, bytes)
	if err != nil {
		return problem.Wrap(err, "encrypt cookie jar")
	}
	return tx.Clauses(clause.OnConflict{Columns: []clause.Column{{Name: "workspace_id"}, {Name: "user_id"}}, DoUpdates: clause.AssignmentColumns([]string{"ciphertext", "revision"})}).Create(&CookieJarRecord{WorkspaceID: workspaceID, UserID: actor.UserID, Ciphertext: ciphertext, Revision: jar.Revision}).Error
}

func (r *cookieJarRepository) Replace(ctx context.Context, actor Actor, workspaceID string, jar CookieJar, reset bool) (CookieJar, error) {
	err := r.core.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := requireCurrentCookieKey(tx, actor); err != nil {
			return err
		}
		var row CookieJarRecord
		err := tx.Clauses(clause.Locking{Strength: "UPDATE"}).Where("workspace_id = ? AND user_id = ?", workspaceID, actor.UserID).First(&row).Error
		if err != nil && !errors.Is(err, gorm.ErrRecordNotFound) {
			return err
		}
		if !reset && jar.Revision != row.Revision {
			return problem.New(problem.KindConflict, "cookie_jar_conflict", "Cookie jar changed on another client. Reload it before saving.")
		}
		jar.Revision = row.Revision + 1
		return r.save(tx, actor, workspaceID, jar)
	})
	return jar, err
}
func (r *cookieJarRepository) Update(ctx context.Context, actor Actor, workspaceID string, mutate func(*CookieJar) (bool, error)) error {
	return r.core.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		jar, err := r.load(tx.Clauses(clause.Locking{Strength: "UPDATE"}), actor, workspaceID)
		if err != nil {
			return err
		}
		changed, err := mutate(&jar)
		if err != nil {
			return err
		}
		if !changed {
			return nil
		}
		jar.Revision++
		return r.save(tx, actor, workspaceID, jar)
	})
}
