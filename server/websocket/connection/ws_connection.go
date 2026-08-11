package connection

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"sync"
	"sync/atomic"
)

var ErrConnectionClosed = errors.New("connection closed")
var ErrWriteBufferFull = errors.New("write buffer full")

type Closable interface {
	Close() error
	Closed() bool
}

type MessageSentHandler func() error

type WebsocketConnection interface {
	Id() string
	Send(ctx context.Context, message []byte, handlers ...MessageSentHandler) error
	SendJson(ctx context.Context, data any, handlers ...MessageSentHandler) error
	ReadMessage(ctx context.Context) ([]byte, error)
	BindJson(ctx context.Context, data any) error
	Local(key string, data any) (any, bool)
	Locals() map[string]any
	Header(key string) (string, bool)
	Closed() bool

	GetRemoteAddr() string
	Context() context.Context
}

type parentConnection interface {
	Send(ctx context.Context, message []byte, handlers ...MessageSentHandler) error
	ReadMessage(ctx context.Context) ([]byte, error)
}

type baseWebsocketConnection struct {
	id     string
	locals sync.Map
	closed *atomic.Bool
	req    *http.Request
	ctx    context.Context
	cancel context.CancelFunc
	parent parentConnection
}

func NewBaseWebsocketConnection(id string, req *http.Request, ctx context.Context, cancel context.CancelFunc) baseWebsocketConnection {
	return baseWebsocketConnection{
		id:     id,
		locals: sync.Map{},
		closed: &atomic.Bool{},
		req:    req,
		ctx:    ctx,
		cancel: cancel,
	}
}

func (base *baseWebsocketConnection) cloneSyncMap() *sync.Map {
	m := sync.Map{}
	base.locals.Range(func(key, value any) bool {
		m.Store(key, value)
		return true
	})
	return &m
}

func (base *baseWebsocketConnection) WithContext(ctx context.Context) baseWebsocketConnection {
	return baseWebsocketConnection{
		id:     base.id,
		locals: *base.cloneSyncMap(),
		closed: base.closed,
		req:    base.req,
		ctx:    ctx,
		parent: base.parent,
		cancel: base.cancel,
	}
}

func (base *baseWebsocketConnection) Context() context.Context {
	return base.ctx
}

func (base *baseWebsocketConnection) Id() string {
	return base.id
}

func (base *baseWebsocketConnection) BindJson(ctx context.Context, data any) error {
	m, e := base.parent.ReadMessage(ctx)
	if e != nil {
		return e
	}

	if e = json.Unmarshal(m, data); e != nil {
		return e
	}
	return nil
}

func (base *baseWebsocketConnection) getLocal(key string) (any, bool) {
	v, ok := base.locals.Load(key)
	return v, ok
}

func (base *baseWebsocketConnection) setLocal(key string, data any) {
	base.locals.Store(key, data)
}

func (base *baseWebsocketConnection) Local(key string, data any) (any, bool) {
	if data == nil {
		return base.getLocal(key)
	}

	base.setLocal(key, data)
	return data, true
}

func (base *baseWebsocketConnection) Header(key string) (string, bool) {
	v := base.req.Header.Get(key)
	return v, v != ""
}

func (base *baseWebsocketConnection) Locals() map[string]any {
	s := make(map[string]any)
	base.locals.Range(func(k, v any) bool {
		s[k.(string)] = v
		return true
	})
	return s
}

func (base *baseWebsocketConnection) tryClose() bool {
	return base.closed.CompareAndSwap(false, true)
}

func (base *baseWebsocketConnection) Closed() bool {
	return base.closed.Load()
}

func (base *baseWebsocketConnection) SendJson(ctx context.Context, data any, handlers ...MessageSentHandler) error {
	jsonBytes, err := json.Marshal(data)
	if err != nil {
		return err
	}

	return base.parent.Send(ctx, jsonBytes, handlers...)
}
