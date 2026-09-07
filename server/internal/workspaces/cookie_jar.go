package workspaces

import (
	"context"
	"crypto/sha256"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"time"

	"gorm.io/gorm"
	"gorm.io/gorm/clause"
	"resolved-server/internal/identity"
	"resolved-server/internal/problem"
	"resolved-server/internal/security"
)

// Cookie jars are private to the signed-in user, even in shared workspaces.
// The deployment data key cannot decrypt their payloads.
type CookieJarRecord struct {
	WorkspaceID string        `gorm:"type:char(36);primaryKey"`
	UserID      string        `gorm:"type:char(36);primaryKey"`
	Workspace   Workspace     `gorm:"foreignKey:WorkspaceID;constraint:OnDelete:CASCADE"`
	User        identity.User `gorm:"foreignKey:UserID;constraint:OnDelete:CASCADE"`
	Ciphertext  []byte        `gorm:"not null"`
	Revision    uint64        `gorm:"not null"`
}
type JarCookie struct {
	URL    string `json:"url"`
	Cookie string `json:"cookie"`
}
type CookieJar struct {
	Enabled  bool        `json:"enabled"`
	Cookies  []JarCookie `json:"cookies"`
	Revision uint64      `json:"revision"`
}

func (s *Service) cookieJars() *cookieJarRepository {
	return &cookieJarRepository{core: s.repository, cipher: s.environmentCipher}
}
func (s *Service) GetCookieJar(ctx context.Context, actor Actor, workspaceID string) (CookieJar, error) {
	if _, err := s.Get(ctx, actor, workspaceID); err != nil {
		return CookieJar{}, err
	}
	return s.cookieJars().Get(ctx, actor, workspaceID)
}
func (s *Service) PutCookieJar(ctx context.Context, actor Actor, workspaceID string, jar CookieJar, reset bool) (CookieJar, error) {
	if _, err := s.Get(ctx, actor, workspaceID); err != nil {
		return CookieJar{}, err
	}
	if len(actor.EnvironmentKey) != security.EnvironmentKeyLength {
		return CookieJar{}, security.ErrEnvironmentKeyUnavailable
	}
	if reset {
		jar.Cookies = []JarCookie{}
	}
	normalized, err := MergeJarCookies(nil, jar.Cookies)
	if err != nil {
		return CookieJar{}, problem.WithFields("validation_failed", "invalid cookie jar", map[string]string{"cookies": err.Error()})
	}
	jar.Cookies = normalized
	return s.cookieJars().Replace(ctx, actor, workspaceID, jar, reset)
}

// Merge response deltas into the latest jar without undoing other clients' edits.
func (s *Service) ApplyCookieUpdates(ctx context.Context, actor Actor, workspaceID string, updates []JarCookie) error {
	if len(updates) == 0 {
		return nil
	}
	if _, err := s.Get(ctx, actor, workspaceID); err != nil {
		return err
	}
	return s.cookieJars().Update(ctx, actor, workspaceID, func(jar *CookieJar) (bool, error) {
		if !jar.Enabled {
			return false, nil
		}
		cookies, err := MergeJarCookies(jar.Cookies, updates)
		if err != nil {
			return false, err
		}
		jar.Cookies = cookies
		return true, nil
	})
}

// Normalize relative expiry and default paths before persistence so reloading
// cannot extend Max-Age or change a cookie's scope.
func MergeJarCookies(existing, updates []JarCookie) ([]JarCookie, error) {
	result := make([]JarCookie, 0, len(existing)+len(updates))
	indexes := map[string]int{}
	for _, item := range append(append([]JarCookie{}, existing...), updates...) {
		if len(item.Cookie) > 4096 || len(item.URL) > 16384 {
			return nil, fmt.Errorf("cookie or origin exceeds its size limit")
		}
		origin, err := url.Parse(item.URL)
		if err != nil || origin.Hostname() == "" || origin.User != nil || (origin.Scheme != "http" && origin.Scheme != "https") {
			return nil, fmt.Errorf("cookie origin must be an HTTP or HTTPS URL without credentials")
		}
		cookie, err := http.ParseSetCookie(item.Cookie)
		if err != nil {
			return nil, fmt.Errorf("invalid Set-Cookie value")
		}
		host := strings.ToLower(origin.Hostname())
		domain := strings.TrimPrefix(strings.ToLower(cookie.Domain), ".")
		if domain == "" {
			domain = host
		} else if host != domain && !strings.HasSuffix(host, "."+domain) {
			return nil, fmt.Errorf("cookie domain does not match origin")
		}
		if cookie.Path == "" || cookie.Path[0] != '/' {
			cookie.Path = "/"
			if i := strings.LastIndex(origin.Path, "/"); i > 0 {
				cookie.Path = origin.Path[:i]
			}
		}
		key := domain + "\x00" + cookie.Path + "\x00" + cookie.Name
		expired := cookie.MaxAge < 0 || cookie.MaxAge == 0 && !cookie.Expires.IsZero() && !cookie.Expires.After(time.Now())
		if cookie.MaxAge > 0 {
			cookie.Expires = time.Now().Add(time.Duration(min(cookie.MaxAge, 315360000)) * time.Second)
			cookie.MaxAge = 0
		}
		if i, ok := indexes[key]; ok {
			result[i] = JarCookie{}
			delete(indexes, key)
		}
		if expired {
			continue
		}
		origin.RawQuery = ""
		origin.Fragment = ""
		origin.Path = cookie.Path
		item = JarCookie{URL: origin.String(), Cookie: cookie.String()}
		indexes[key] = len(result)
		result = append(result, item)
	}
	compact := make([]JarCookie, 0, len(result))
	for _, item := range result {
		if item.URL != "" {
			compact = append(compact, item)
		}
	}
	if len(compact) > 512 {
		return nil, fmt.Errorf("cookie jar exceeds 512 cookies")
	}
	return compact, nil
}
func RekeyCookieJars(ctx context.Context, tx *gorm.DB, cipher *security.EnvironmentCipher, userID string, oldKey, newKey []byte) error {
	var rows []CookieJarRecord
	if err := tx.WithContext(ctx).Where("user_id = ?", userID).Find(&rows).Error; err != nil {
		return err
	}
	if len(rows) > 0 && len(oldKey) != security.EnvironmentKeyLength {
		return security.ErrEnvironmentKeyUnavailable
	}
	for _, row := range rows {
		plaintext, err := cipher.DecryptCookieJar(oldKey, userID, row.WorkspaceID, row.Ciphertext)
		if err != nil {
			return err
		}
		ciphertext, err := cipher.EncryptCookieJar(newKey, userID, row.WorkspaceID, plaintext)
		clear(plaintext)
		if err != nil {
			return err
		}
		if err := tx.Model(&CookieJarRecord{}).Where("workspace_id = ? AND user_id = ?", row.WorkspaceID, userID).Update("ciphertext", ciphertext).Error; err != nil {
			return err
		}
	}
	return nil
}

// Lock the user before the jar, matching password-change transaction order.
// An in-flight request holding the previous password key cannot overwrite a
// rekeyed jar, including through the explicit reset endpoint.
func requireCurrentCookieKey(db *gorm.DB, actor Actor) error {
	var user identity.User
	if err := db.Session(&gorm.Session{}).Clauses(clause.Locking{Strength: "UPDATE"}).Select("id", "password_hash", "active").Where("id = ?", actor.UserID).First(&user).Error; err != nil {
		return err
	}
	if !user.Active || sha256.Sum256([]byte(user.PasswordHash)) != actor.CredentialVersion {
		return problem.New(problem.KindUnauthorized, "cookie_key_unavailable", "Log in again to unlock cookie storage.")
	}
	return nil
}
