package server

import (
	"context"
	"log"
	"net"
	"net/http"
	"sync"
	"time"

	"github.com/gofiber/fiber/v3"
	"github.com/gofiber/fiber/v3/middleware/adaptor"
	gorillaWebsocket "github.com/gorilla/websocket"

	"resolved-server/websocket/connection"
)

const (
	maxWebsocketConnectionsPerIP = 20
	maxWebsocketTotalConnections  = 500
	controlWriteTimeout          = 5 * time.Second
)

type GorillaWebsocketServer struct {
	baseWebsocketServer
	upgrader *gorillaWebsocket.Upgrader
	// Per-remote-IP and total connection budgets to bound zombie-socket
	// resource use (there was previously no connection cap).
	connMutex  sync.Mutex
	connCounts map[string]int
}

var _ WebsocketServer = (*GorillaWebsocketServer)(nil)

type ConcurrentWriter interface {
	WriteLoop()
}

func (base *baseWebsocketServer) UserCount() uint64 {
	l := base.connectedUsers.Len()
	return uint64(l)
}

func MakeGorillaWebsocketServer() WebsocketServer {
	g := &GorillaWebsocketServer{
		upgrader: &gorillaWebsocket.Upgrader{
			EnableCompression: true,
			HandshakeTimeout:  3 * time.Second,
		},
		baseWebsocketServer: NewDefaultBaseWebsocketServerImplementation(),
		connCounts:          make(map[string]int),
	}
	g.acceptedAtLeastOneConnection.Store(false)
	return g
}

func (g *GorillaWebsocketServer) reserveConnection(remoteIP string) bool {
	g.connMutex.Lock()
	defer g.connMutex.Unlock()
	total := 0
	for _, count := range g.connCounts {
		total += count
	}
	if total >= maxWebsocketTotalConnections || g.connCounts[remoteIP] >= maxWebsocketConnectionsPerIP {
		return false
	}
	g.connCounts[remoteIP]++
	return true
}

func (g *GorillaWebsocketServer) releaseConnection(remoteIP string) {
	g.connMutex.Lock()
	defer g.connMutex.Unlock()
	if g.connCounts[remoteIP] <= 1 {
		delete(g.connCounts, remoteIP)
		return
	}
	g.connCounts[remoteIP]--
}

func remoteIP(r *http.Request) string {
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return r.RemoteAddr
	}
	return host
}

func (g *GorillaWebsocketServer) FiberHandler() fiber.Handler {
	return adaptor.HTTPHandlerWithContext(http.HandlerFunc(g.AcceptWebsocket))
}

func (g *GorillaWebsocketServer) AcceptWebsocket(w http.ResponseWriter, r *http.Request) {
	c, err := g.upgrader.Upgrade(w, r, nil)
	if err != nil {
		log.Println(err)
		return
	}
	if !g.reserveConnection(remoteIP(r)) {
		log.Printf("rejecting websocket connection from %s: connection limit reached", remoteIP(r))
		_ = c.Close()
		return
	}
	g.acceptedAtLeastOneConnection.Store(true)
	c.SetReadLimit(g.ReadLimit())

	// Refresh the read deadline on any traffic (including ping/pong control
	// frames) so healthy-but-quiet peers stay alive while vanished ones are
	// reaped by the per-read deadline in ReadMessage.
	c.SetReadDeadline(time.Now().Add(connection.ReadIdleTimeout))
	c.SetPongHandler(func(string) error {
		return c.SetReadDeadline(time.Now().Add(connection.ReadIdleTimeout))
	})
	c.SetPingHandler(func(appData string) error {
		if err := c.WriteControl(
			gorillaWebsocket.PongMessage,
			[]byte(appData),
			time.Now().Add(controlWriteTimeout),
		); err != nil {
			return err
		}
		return c.SetReadDeadline(time.Now().Add(connection.ReadIdleTimeout))
	})

	connectionContext := context.Background()
	if fiberContext, ok := adaptor.LocalContextFromHTTPRequest(r); ok {
		connectionContext = fiberContext
	}
	originalCon := connection.NewGorillaWebsocketConnectionWithContext(c, r, connectionContext)
	ip := remoteIP(r)
	var safeCon *safeClosableWebsocketConnection
	safeCon = &safeClosableWebsocketConnection{
		WebsocketConnection: originalCon,
		singleClosable: sync.OnceFunc(func() {
			g.releaseConnection(ip)
			g.closeConnection(safeCon)
		}),
	}

	for _, f := range g.channelActivePipeline {
		f(safeCon)
	}

	if safeCon.Closed() {
		log.Println("connection closed by one of the previous handlers or user.")
		return
	}

	if c, ok := safeCon.WebsocketConnection.(ConcurrentWriter); ok {
		go func() {
			defer g.CloseConnection(safeCon)
			c.WriteLoop()
		}()
		go g.IncomingCommand(safeCon)
	} else {
		go func() {
			defer g.CloseConnection(safeCon)
			g.IncomingCommand(safeCon)
		}()
	}
}
