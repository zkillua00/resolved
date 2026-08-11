package server

import (
	"context"
	"log"
	"net/http"
	"sync"
	"time"

	"github.com/gofiber/fiber/v3"
	"github.com/gofiber/fiber/v3/middleware/adaptor"
	gorillaWebsocket "github.com/gorilla/websocket"

	"resolved-server/websocket/connection"
)

type GorillaWebsocketServer struct {
	baseWebsocketServer
	upgrader *gorillaWebsocket.Upgrader
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
			CheckOrigin: func(r *http.Request) bool {
				return true
			},
		},
		baseWebsocketServer: NewDefaultBaseWebsocketServerImplementation(),
	}
	g.acceptedAtLeastOneConnection.Store(false)
	return g
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
	g.acceptedAtLeastOneConnection.Store(true)
	c.SetReadLimit(g.ReadLimit())

	connectionContext := context.Background()
	if fiberContext, ok := adaptor.LocalContextFromHTTPRequest(r); ok {
		connectionContext = fiberContext
	}
	originalCon := connection.NewGorillaWebsocketConnectionWithContext(c, r, connectionContext)
	var safeCon *safeClosableWebsocketConnection
	safeCon = &safeClosableWebsocketConnection{
		WebsocketConnection: originalCon,
		singleClosable: sync.OnceFunc(func() {
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
