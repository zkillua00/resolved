package security

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"path"
	"strings"
	"time"
)

const maxVaultResponseBytes = 1024 * 1024

type VaultTransitKeyProvider struct {
	address   *url.URL
	token     string
	namespace string
	mount     string
	keyName   string
	keyID     string
	client    *http.Client
}

func NewVaultTransitKeyProvider(
	address, token, namespace, mount, keyName string,
) (*VaultTransitKeyProvider, error) {
	parsed, err := url.Parse(strings.TrimSpace(address))
	if err != nil || parsed.Scheme == "" || parsed.Host == "" || parsed.User != nil || parsed.RawQuery != "" || parsed.Fragment != "" {
		return nil, errors.New("RESOLVED_VAULT_ADDRESS must be an absolute HTTP or HTTPS URL without credentials, query, or fragment")
	}
	if parsed.Scheme != "https" && !(parsed.Scheme == "http" && isLoopbackHost(parsed.Hostname())) {
		return nil, errors.New("RESOLVED_VAULT_ADDRESS must use HTTPS unless it points to loopback")
	}
	if strings.TrimSpace(token) == "" {
		return nil, errors.New("RESOLVED_VAULT_TOKEN is required for the Vault data key provider")
	}
	mount = strings.Trim(strings.TrimSpace(mount), "/")
	keyName = strings.Trim(strings.TrimSpace(keyName), "/")
	if !validVaultPathSegment(mount) || !validVaultPathSegment(keyName) {
		return nil, errors.New("Vault transit mount and key name may contain only letters, numbers, dash, and underscore")
	}
	parsed.Path = strings.TrimSuffix(parsed.Path, "/")
	return &VaultTransitKeyProvider{
		address: parsed, token: token, namespace: strings.TrimSpace(namespace),
		mount: mount, keyName: keyName, keyID: "vault:" + mount + ":" + keyName,
		client: &http.Client{Timeout: 10 * time.Second},
	}, nil
}

func (p *VaultTransitKeyProvider) WrapKey(
	ctx context.Context,
	plaintext, additionalData []byte,
) (string, []byte, error) {
	var response struct {
		Data struct {
			Ciphertext string `json:"ciphertext"`
		} `json:"data"`
	}
	err := p.call(ctx, "encrypt", map[string]string{
		"plaintext":       base64.StdEncoding.EncodeToString(plaintext),
		"associated_data": base64.StdEncoding.EncodeToString(additionalData),
	}, &response)
	if err != nil {
		return "", nil, err
	}
	if response.Data.Ciphertext == "" {
		return "", nil, errors.New("Vault Transit returned an empty wrapped key")
	}
	return p.keyID, []byte(response.Data.Ciphertext), nil
}

func (p *VaultTransitKeyProvider) UnwrapKey(
	ctx context.Context,
	providerKeyID string,
	wrapped, additionalData []byte,
) ([]byte, error) {
	if providerKeyID != p.keyID {
		return nil, fmt.Errorf("%w: provider key %q is not configured", ErrDataKeyUnavailable, providerKeyID)
	}
	var response struct {
		Data struct {
			Plaintext string `json:"plaintext"`
		} `json:"data"`
	}
	err := p.call(ctx, "decrypt", map[string]string{
		"ciphertext":      string(wrapped),
		"associated_data": base64.StdEncoding.EncodeToString(additionalData),
	}, &response)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", ErrDataKeyUnavailable, err)
	}
	plaintext, err := base64.StdEncoding.DecodeString(response.Data.Plaintext)
	if err != nil {
		return nil, fmt.Errorf("%w: Vault Transit returned invalid plaintext", ErrDataKeyUnavailable)
	}
	return plaintext, nil
}

func (p *VaultTransitKeyProvider) call(
	ctx context.Context,
	operation string,
	payload any,
	target any,
) error {
	body, err := json.Marshal(payload)
	if err != nil {
		return fmt.Errorf("encode Vault Transit request: %w", err)
	}
	endpoint := *p.address
	endpoint.Path = path.Join(endpoint.Path, "v1", p.mount, operation, p.keyName)
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint.String(), bytes.NewReader(body))
	if err != nil {
		return fmt.Errorf("create Vault Transit request: %w", err)
	}
	request.Header.Set("Content-Type", "application/json")
	request.Header.Set("X-Vault-Token", p.token)
	if p.namespace != "" {
		request.Header.Set("X-Vault-Namespace", p.namespace)
	}
	response, err := p.client.Do(request)
	if err != nil {
		return fmt.Errorf("call Vault Transit: %w", err)
	}
	defer response.Body.Close()
	limited := io.LimitReader(response.Body, maxVaultResponseBytes+1)
	responseBody, err := io.ReadAll(limited)
	if err != nil {
		return fmt.Errorf("read Vault Transit response: %w", err)
	}
	if len(responseBody) > maxVaultResponseBytes {
		return errors.New("Vault Transit response exceeds the size limit")
	}
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		return fmt.Errorf("Vault Transit returned HTTP %d", response.StatusCode)
	}
	if err := json.Unmarshal(responseBody, target); err != nil {
		return fmt.Errorf("decode Vault Transit response: %w", err)
	}
	return nil
}

func isLoopbackHost(host string) bool {
	if strings.EqualFold(host, "localhost") {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

func validVaultPathSegment(value string) bool {
	if value == "" {
		return false
	}
	for _, character := range value {
		if character >= 'a' && character <= 'z' || character >= 'A' && character <= 'Z' ||
			character >= '0' && character <= '9' || character == '-' || character == '_' {
			continue
		}
		return false
	}
	return true
}
