package requestproxy

import (
	"context"
	"io"
	"math"
	"net"
	"net/http"
	"time"

	"resolved-server/internal/executionlimits"
	"resolved-server/internal/workspaces"
)

type executionSnapshot struct {
	workspaceID, collectionID string
	snapshot                  executionlimits.Snapshot
}
type executionSnapshotKey struct{}

func executionContext(ctx context.Context, bound executionlimits.Bound) (context.Context, context.CancelFunc) {
	if bound.Unlimited {
		return context.WithCancel(ctx)
	}
	return context.WithTimeout(ctx, limitDuration(bound))
}

func executionTransport(base *http.Transport, snapshot executionlimits.Snapshot) *http.Transport {
	transport := base.Clone()
	transport.DialContext = (&net.Dialer{Timeout: limitDuration(snapshot.Effective["http.connect_timeout_ms"]), KeepAlive: 30 * time.Second}).DialContext
	transport.TLSHandshakeTimeout = limitDuration(snapshot.Effective["http.tls_handshake_timeout_ms"])
	transport.ResponseHeaderTimeout = 0
	transport = transportWithHostnameOverrides(transport)
	dial := transport.DialContext
	transport.DialContext = func(ctx context.Context, network, address string) (net.Conn, error) {
		// Include the validated DNS lookup, not only the final TCP dial.
		connectContext, cancel := executionContext(ctx, snapshot.Effective["http.connect_timeout_ms"])
		defer cancel()
		return dial(connectContext, network, address)
	}
	return transport
}

func (s *Service) resolveLimits(ctx context.Context, actor workspaces.Actor, workspaceID, collectionID string) (executionlimits.Snapshot, error) {
	if cached, ok := ctx.Value(executionSnapshotKey{}).(executionSnapshot); ok && cached.workspaceID == workspaceID && cached.collectionID == collectionID {
		return cached.snapshot, nil
	}
	if collectionID != "" {
		if workspaceID == "" {
			return executionlimits.Snapshot{}, invalidField("workspace_id", "is required with collection_id")
		}
		if _, err := s.workspaces.GetCollection(ctx, actor, workspaceID, collectionID); err != nil {
			return executionlimits.Snapshot{}, err
		}
	} else if workspaceID != "" {
		if _, err := s.workspaces.Get(ctx, actor, workspaceID); err != nil {
			return executionlimits.Snapshot{}, err
		}
	}
	return s.limits.Resolve(ctx, executionlimits.Scope{WorkspaceID: workspaceID, CollectionID: collectionID})
}

func exceeds(bound executionlimits.Bound, size int64) bool {
	return !bound.Unlimited && size > bound.Value
}

func limitDuration(bound executionlimits.Bound) time.Duration {
	if bound.Unlimited {
		return 0
	}
	if bound.Value > math.MaxInt64/int64(time.Millisecond) {
		return time.Duration(math.MaxInt64)
	}
	// Go networking APIs use zero to disable deadlines.
	if bound.Value == 0 {
		return time.Nanosecond
	}
	return time.Duration(bound.Value) * time.Millisecond
}

func readLimited(reader io.Reader, bound executionlimits.Bound) ([]byte, bool, error) {
	if bound.Unlimited || bound.Value == math.MaxInt64 {
		body, err := io.ReadAll(reader)
		return body, false, err
	}
	body, err := io.ReadAll(io.LimitReader(reader, bound.Value+1))
	return body, exceeds(bound, int64(len(body))), err
}

func validateDescriptor(snapshot executionlimits.Snapshot, target string, headers []Header) error {
	if exceeds(snapshot.Effective["http.url_bytes"], int64(len(target))) {
		return invalidField("url", "is too long")
	}
	if exceeds(snapshot.Effective["http.header_count"], int64(len(headers))) {
		return invalidField("headers", "contains too many entries")
	}
	return nil
}
