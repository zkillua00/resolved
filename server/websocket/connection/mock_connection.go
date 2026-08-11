package connection

import (
	"context"
	"log"
	"net/http"
	"sync"

	"github.com/google/uuid"
)

type MockWebsocketConnection struct {
	baseWebsocketConnection
	ReceivedMessages [][]byte
	Mu               sync.Mutex
}

func NewMockWebsocketConnection() *MockWebsocketConnection {
	ctx, cancel := context.WithCancel(context.Background())
	id := uuid.NewString()
	m := &MockWebsocketConnection{
		baseWebsocketConnection: NewBaseWebsocketConnection(id, &http.Request{Header: http.Header{}}, ctx, cancel),
		ReceivedMessages:        make([][]byte, 0),
	}
	m.parent = m
	return m
}

func (m *MockWebsocketConnection) ReadMessage(ctx context.Context) ([]byte, error) {
	// Block forever as if waiting for input
	select {}
}

func (m *MockWebsocketConnection) Send(ctx context.Context, message []byte, handlers ...MessageSentHandler) error {
	m.Mu.Lock()
	defer m.Mu.Unlock()
	m.ReceivedMessages = append(m.ReceivedMessages, message)

	for _, handler := range handlers {
		if err := handler(); err != nil {
			return err
		}
	}

	log.Printf("MockConnection %s received: %s", m.id, string(message))
	return nil
}

func (m *MockWebsocketConnection) Close() error {
	m.closed.Store(true)
	return nil
}

func (m *MockWebsocketConnection) GetRemoteAddr() string {
	return "mock"
}
