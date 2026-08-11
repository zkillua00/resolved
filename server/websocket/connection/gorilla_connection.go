package connection

import (
	"context"
	"errors"
	"log"
	"net/http"
	"sync/atomic"

	"github.com/google/uuid"
	gorillaWebsocket "github.com/gorilla/websocket"
)

const gorillaWriteBufferSize = 4096

type messageWithHandlers struct {
	message  []byte
	handlers []MessageSentHandler
	ctx      context.Context
}

type GorillaWebsocketConnection struct {
	con             *gorillaWebsocket.Conn
	writeBuffer     chan messageWithHandlers
	closedChan      chan struct{}
	closeChanClosed *atomic.Bool
	baseWebsocketConnection
}

func NewGorillaWebsocketConnection(con *gorillaWebsocket.Conn, req *http.Request) WebsocketConnection {
	return NewGorillaWebsocketConnectionWithContext(con, req, context.Background())
}

func NewGorillaWebsocketConnectionWithContext(con *gorillaWebsocket.Conn, req *http.Request, parent context.Context) WebsocketConnection {
	if parent == nil {
		parent = context.Background()
	}
	ctx, cancel := context.WithCancel(parent)

	g := &GorillaWebsocketConnection{
		con:                     con,
		writeBuffer:             make(chan messageWithHandlers, gorillaWriteBufferSize),
		closedChan:              make(chan struct{}),
		closeChanClosed:         &atomic.Bool{},
		baseWebsocketConnection: NewBaseWebsocketConnection(uuid.NewString(), req, ctx, cancel),
	}
	g.parent = g
	return g
}

func (g *GorillaWebsocketConnection) WithContext(ctx context.Context) WebsocketConnection {
	return &GorillaWebsocketConnection{
		con:                     g.con,
		writeBuffer:             g.writeBuffer, // share channels and buffers with the original connection since the gorilla websocket connection is not thread safe
		closedChan:              g.closedChan,
		closeChanClosed:         g.closeChanClosed,
		baseWebsocketConnection: g.baseWebsocketConnection.WithContext(ctx),
	}
}

func (g *GorillaWebsocketConnection) GetRemoteAddr() string {
	return g.con.RemoteAddr().String()
}

func (g *GorillaWebsocketConnection) ReadMessage(ctx context.Context) ([]byte, error) {
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	default:
		_, m, e := g.con.ReadMessage()
		return m, e
	}
}

func (g *GorillaWebsocketConnection) tryTearingConnection() bool {
	if err := g.con.Close(); err != nil {
		log.Printf("Error closing connection: %v, this is probably not an actual error. this is most of the time expected.", err)
		return false
	}
	return true
}

func (g *GorillaWebsocketConnection) tryClosingCloseChannel() bool {
	if !g.closeChanClosed.CompareAndSwap(false, true) {
		return false
	}
	close(g.closedChan)
	return true
}

func (g *GorillaWebsocketConnection) Close() error {
	if !g.tryClose() {
		log.Println("connection already closed")
		return nil
	}
	if !g.tryClosingCloseChannel() {
		log.Println("WARN: recovered from race. the system was about to panic due to double closure!")
	}

	if g.cancel != nil {
		g.cancel()
	}

	return g.con.Close()
}

func (g *GorillaWebsocketConnection) Send(ctx context.Context, message []byte, handlers ...MessageSentHandler) (err error) {
	if g.Closed() {
		err = ErrConnectionClosed
		recordConnectionError(g.Id(), g.GetRemoteAddr(), "send", err)
		return ErrConnectionClosed
	}

	select {
	case g.writeBuffer <- messageWithHandlers{
		message:  message,
		handlers: handlers,
		ctx:      ctx,
	}:
		{
			return nil
		}
	case <-g.closedChan:
		recordConnectionError(g.Id(), g.GetRemoteAddr(), "send", ErrConnectionClosed)
		return ErrConnectionClosed
	case <-ctx.Done():
		recordConnectionError(g.Id(), g.GetRemoteAddr(), "send", ctx.Err())
		return ctx.Err()
	default:
		recordConnectionError(g.Id(), g.GetRemoteAddr(), "send", ErrWriteBufferFull)
		return ErrWriteBufferFull
	}
}

func (g *GorillaWebsocketConnection) WriteLoop() {
	defer func() {
		if !g.tryTearingConnection() {
			log.Println("NOTE: connection already closed. this is not an error.")
		}
	}()

	for {
		select {
		case <-g.closedChan:
			return
		case message := <-g.writeBuffer:
			select {
			case <-message.ctx.Done():
				recordConnectionError(g.Id(), g.GetRemoteAddr(), "write", message.ctx.Err())
				continue
			default:
			}

			if err := g.con.WriteMessage(gorillaWebsocket.TextMessage, message.message); err != nil {
				recordConnectionError(g.Id(), g.GetRemoteAddr(), "write", err)
				if !g.tryClosingCloseChannel() {
					log.Println("WARN: recovered from race. the system was about to panic due to double closure!")
				}
				return
			}

			go func() {
				for _, handler := range message.handlers {
					if err := handler(); err != nil {
						recordConnectionError(g.Id(), g.GetRemoteAddr(), "send_handler", err)
						if errors.Is(err, ErrConnectionClosed) {
							return
						}
						if !g.tryClosingCloseChannel() {
							log.Println("WARN: recovered from race. the system was about to panic due to double closure!")
						}
						return
					}
				}
			}()

		}
	}
}
