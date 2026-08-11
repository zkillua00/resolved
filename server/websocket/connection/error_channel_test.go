package connection

import (
	"errors"
	"testing"
)

func TestConnectionErrorChannelFiltersClosedErrors(t *testing.T) {
	DrainConnectionErrors(0)

	recordConnectionError("closed", "127.0.0.1:1", "send", ErrConnectionClosed)
	recordConnectionError("buffer", "127.0.0.1:2", "send", ErrWriteBufferFull)

	errs := DrainConnectionErrors(0)
	if len(errs) != 1 {
		t.Fatalf("drained errors = %d, want 1", len(errs))
	}
	if errs[0].ConnectionID != "buffer" || errs[0].Error != ErrWriteBufferFull.Error() {
		t.Fatalf("drained error = %+v, want write-buffer error", errs[0])
	}
}

func TestConnectionErrorChannelFiltersCommonClosedNetworkErrors(t *testing.T) {
	DrainConnectionErrors(0)

	recordConnectionError("closed", "127.0.0.1:1", "write", errors.New("use of closed network connection"))

	if errs := DrainConnectionErrors(0); len(errs) != 0 {
		t.Fatalf("drained errors = %d, want 0", len(errs))
	}
}
