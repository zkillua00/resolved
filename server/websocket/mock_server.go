package server

import (
	"net/http"
	"sync/atomic"

	"github.com/gofiber/fiber/v3"
	"github.com/gofiber/fiber/v3/middleware/adaptor"

	"resolved-server/websocket/connection"
)

type MockWebsocketServer struct {
	baseWebsocketServer
}

var _ WebsocketServer = (*MockWebsocketServer)(nil)

func MakeMockWebsocketServer() *MockWebsocketServer {
	m := &MockWebsocketServer{
		baseWebsocketServer: baseWebsocketServer{
			commandHandlers:              make(map[string]func(connection.WebsocketConnection, JsonAny)),
			channelActivePipeline:        ActivePipeline{},
			channelInactivePipeline:      InActivePipeline{},
			connectedUsers:               ConnectedUsers{},
			acceptedAtLeastOneConnection: atomic.Bool{},
			readLimit:                    1024 * 1024 * 10,
		},
	}
	m.acceptedAtLeastOneConnection.Store(false)
	return m
}

func (m *MockWebsocketServer) AcceptWebsocket(w http.ResponseWriter, r *http.Request) {
	// Mock implementation: does nothing
}

func (m *MockWebsocketServer) FiberHandler() fiber.Handler {
	return adaptor.HTTPHandlerWithContext(http.HandlerFunc(m.AcceptWebsocket))
}
