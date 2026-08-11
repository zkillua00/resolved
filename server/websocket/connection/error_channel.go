package connection

import (
	"errors"
	"log"
	"net"
	"strings"
	"time"

	gorillaWebsocket "github.com/gorilla/websocket"
)

const connectionErrorBufferSize = 4096

type ConnectionError struct {
	ConnectionID string    `json:"connection_id"`
	RemoteAddr   string    `json:"remote_addr"`
	Operation    string    `json:"operation"`
	Error        string    `json:"error"`
	At           time.Time `json:"at"`
}

var connectionErrors = make(chan ConnectionError, connectionErrorBufferSize)

func recordConnectionError(connectionID string, remoteAddr string, operation string, err error) {
	if err == nil {
		return
	}

	log.Printf("[connection:%s] %s failed for %s: %v", connectionID, operation, remoteAddr, err)
	if isClosedConnectionError(err) {
		return
	}

	entry := ConnectionError{
		ConnectionID: connectionID,
		RemoteAddr:   remoteAddr,
		Operation:    operation,
		Error:        err.Error(),
		At:           time.Now().UTC(),
	}

	select {
	case connectionErrors <- entry:
		return
	default:
	}

	select {
	case <-connectionErrors:
	default:
	}

	select {
	case connectionErrors <- entry:
	default:
	}
}

func DrainConnectionErrors(limit int) []ConnectionError {
	if limit <= 0 || limit > len(connectionErrors) {
		limit = len(connectionErrors)
	}

	errs := make([]ConnectionError, 0, limit)
	for len(errs) < limit {
		select {
		case err := <-connectionErrors:
			errs = append(errs, err)
		default:
			return errs
		}
	}
	return errs
}

func isClosedConnectionError(err error) bool {
	if errors.Is(err, ErrConnectionClosed) || errors.Is(err, net.ErrClosed) {
		return true
	}

	if gorillaWebsocket.IsCloseError(
		err,
		gorillaWebsocket.CloseNormalClosure,
		gorillaWebsocket.CloseGoingAway,
		gorillaWebsocket.CloseNoStatusReceived,
		gorillaWebsocket.CloseAbnormalClosure,
	) {
		return true
	}

	msg := strings.ToLower(err.Error())
	return strings.Contains(msg, "use of closed network connection") ||
		strings.Contains(msg, "websocket: close")
}
