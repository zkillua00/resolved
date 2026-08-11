package security

import (
	"errors"
	"sync"
	"time"
)

var ErrEnvironmentKeyUnavailable = errors.New("environment key is unavailable")

type sessionEnvironmentKey struct {
	userID         string
	credentialHash string
	key            []byte
	expiresAt      time.Time
	generation     uint64
}

// SessionEnvironmentKeys holds derived environment keys only in this server
// process. Token hashes bind keys to individual authenticated sessions without
// putting key material into the database or bearer token.
type SessionEnvironmentKeys struct {
	mu             sync.Mutex
	entries        map[string]sessionEnvironmentKey
	nextGeneration uint64
}

func NewSessionEnvironmentKeys() *SessionEnvironmentKeys {
	return &SessionEnvironmentKeys{entries: make(map[string]sessionEnvironmentKey)}
}

func (s *SessionEnvironmentKeys) Put(
	tokenHash, userID, credentialHash string,
	key []byte,
	expiresAt time.Time,
) error {
	if s == nil || tokenHash == "" || userID == "" || credentialHash == "" || len(key) != EnvironmentKeyLength {
		return ErrEnvironmentKeyUnavailable
	}
	s.mu.Lock()
	s.purgeExpiredLocked(time.Now().UTC())
	if existing, ok := s.entries[tokenHash]; ok {
		clear(existing.key)
	}
	s.nextGeneration++
	generation := s.nextGeneration
	s.entries[tokenHash] = sessionEnvironmentKey{
		userID:         userID,
		credentialHash: credentialHash,
		key:            append([]byte(nil), key...),
		expiresAt:      expiresAt,
		generation:     generation,
	}
	s.mu.Unlock()

	delay := time.Until(expiresAt)
	if delay < 0 {
		delay = 0
	}
	time.AfterFunc(delay, func() {
		s.deleteGeneration(tokenHash, generation)
	})
	return nil
}

func (s *SessionEnvironmentKeys) Get(tokenHash, userID, credentialHash string) ([]byte, bool) {
	if s == nil {
		return nil, false
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.purgeExpiredLocked(time.Now().UTC())
	entry, ok := s.entries[tokenHash]
	if !ok || entry.userID != userID {
		return nil, false
	}
	if entry.credentialHash != credentialHash {
		clear(entry.key)
		delete(s.entries, tokenHash)
		return nil, false
	}
	return append([]byte(nil), entry.key...), true
}

func (s *SessionEnvironmentKeys) AnyForUser(userID, credentialHash string) ([]byte, bool) {
	if s == nil {
		return nil, false
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.purgeExpiredLocked(time.Now().UTC())
	for _, entry := range s.entries {
		if entry.userID == userID && entry.credentialHash == credentialHash {
			return append([]byte(nil), entry.key...), true
		}
	}
	return nil, false
}

func (s *SessionEnvironmentKeys) Delete(tokenHash string) {
	if s == nil {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if entry, ok := s.entries[tokenHash]; ok {
		clear(entry.key)
		delete(s.entries, tokenHash)
	}
}

func (s *SessionEnvironmentKeys) DeleteUser(userID string) {
	if s == nil {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	for tokenHash, entry := range s.entries {
		if entry.userID == userID {
			clear(entry.key)
			delete(s.entries, tokenHash)
		}
	}
}

func (s *SessionEnvironmentKeys) purgeExpiredLocked(now time.Time) {
	for tokenHash, entry := range s.entries {
		if !entry.expiresAt.After(now) {
			clear(entry.key)
			delete(s.entries, tokenHash)
		}
	}
}

func (s *SessionEnvironmentKeys) deleteGeneration(tokenHash string, generation uint64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	entry, ok := s.entries[tokenHash]
	if !ok || entry.generation != generation || entry.expiresAt.After(time.Now().UTC()) {
		return
	}
	clear(entry.key)
	delete(s.entries, tokenHash)
}
