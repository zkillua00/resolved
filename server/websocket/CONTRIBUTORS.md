# Contributing a New WebSocket Backend

The server package keeps its connection and lifecycle code independent from a
specific WebSocket engine. Concrete libraries such as Gorilla, gws, or
Coder/websocket stay behind thin adapters, while Fiber v3 is the application
server boundary.

This guide explains how to add a backend implementation that plugs into the
shared connection, lifecycle, command, user, channel, and metrics code.

The Gorilla implementation is exposed to Fiber v3 as a regular handler:

```go
ws := server.MakeGorillaWebsocketServer()
app.Get("/ws", ws.FiberHandler())
```

`AcceptWebsocket` remains available for existing `net/http` integrations.

---

## 1. Architecture Overview

### Core connection interfaces

```go
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
```

- `WebsocketConnection` is the public connection interface used by handlers
  and higher-level application code.
- `Closable` is intentionally not embedded into `WebsocketConnection`.
  Connections still must implement it, but callers should close connections
  through `server.CloseConnection(conn)` so inactive handlers run correctly.
- `parentConnection` is used internally by `baseWebsocketConnection` for
  `SendJson` and `BindJson`. A concrete connection normally implements both
  `WebsocketConnection` and `parentConnection`.
- Every backend should respect the `context.Context` passed to `Send`,
  `SendJson`, `ReadMessage`, and `BindJson`.

### Shared base connection

```go
type baseWebsocketConnection struct {
	id     string
	locals sync.Map
	closed *atomic.Bool
	req    *http.Request
	ctx    context.Context
	cancel context.CancelFunc
	parent parentConnection
}
```

`baseWebsocketConnection` already implements:

- `Id`
- `Context`
- `WithContext` on the base type
- `BindJson`
- `SendJson`
- `Header`
- `Local` and `Locals`
- `Closed` and `tryClose`

Your backend connection embeds this type and implements only the
transport-specific pieces: `Send`, `ReadMessage`, `Close`, `GetRemoteAddr`,
and optionally a backend-specific `WithContext`.

### Shared server base

```go
type baseWebsocketServer struct {
	connectedUsers               ConnectedUsers
	channelUsers                 _map.SynchronizedMap[string, *ChannelHub]
	commandHandlers              map[string]func(connection.WebsocketConnection, JsonAny)
	channelActivePipeline        ActivePipeline
	channelInactivePipeline      InActivePipeline
	acceptedAtLeastOneConnection atomic.Bool
	readLimit                    int64

	atomicCounters counters.AtomicCounters
}
```

The public server interface is:

```go
type WebsocketServer interface {
	AcceptWebsocket(w http.ResponseWriter, r *http.Request)
	FiberHandler() fiber.Handler
	IncomingCommand(gcon connection.WebsocketConnection)
	AddCommandHandler(command string, handler func(connection.WebsocketConnection, JsonAny))
	AddOnActiveHandler(f ActiveHandler)
	AddOnInactiveHandler(f InActiveHandler)
	RemoveCommandHandler(command string)

	AddUser(id string, channel string, con connection.WebsocketConnection)
	RemoveUser(con connection.WebsocketConnection, id string, channel string)
	GetUserConnection(id string, channel string) ([]connection.WebsocketConnection, bool)
	GetUserConnections(id string) (*UserConnections, bool)

	GetChannel(channel string) (*ChannelHub, bool)

	UserCount() uint64
	CloseConnection(con connection.WebsocketConnection)
	Broadcast(message []byte, channel *string)
	BroadcastJson(data interface{}, channel *string)
	SetReadLimit(limit int64)
	ReadLimit() int64

	AtomicCounters() counters.AtomicCounters
	AtomicCounter(field string) counters.AtomicCounter

	Metrics() Metrics
}
```

`baseWebsocketServer` implements the shared logic for command handlers,
pipelines, users, channels, connection closing, broadcasting, read-limit
storage, and metrics counters. A backend server normally embeds
`baseWebsocketServer` and implements `AcceptWebsocket` plus `FiberHandler` if
the default `IncomingCommand` loop is suitable.

---

## 2. What You Need to Implement

For a new backend, add two concrete types:

- A connection type that embeds `baseWebsocketConnection` and implements
  `connection.WebsocketConnection`, `parentConnection`, and `connection.Closable`.
- A server type that embeds `baseWebsocketServer` and implements
  `AcceptWebsocket` and `FiberHandler`.

### 2.1 Connection type

Responsibilities:

- Embed `baseWebsocketConnection`.
- Initialize it with `NewBaseWebsocketConnection(id, req, ctx, cancel)`.
- Set the reverse parent reference: `conn.parent = conn`.
- Implement:
  - `Send(ctx context.Context, []byte, ...MessageSentHandler) error`
  - `ReadMessage(ctx context.Context) ([]byte, error)`
  - `Close() error`
  - `GetRemoteAddr() string`
- Return `connection.ErrConnectionClosed` when a send/read is attempted after
  closure where the backend can detect that condition.
- Call message-sent handlers only after the message has actually been written.

Typical pattern:

```go
type FooWebsocketConnection struct {
	raw *foo.Conn
	baseWebsocketConnection
}

func NewFooWebsocketConnection(raw *foo.Conn, req *http.Request) connection.WebsocketConnection {
	ctx, cancel := context.WithCancel(context.Background())

	conn := &FooWebsocketConnection{
		raw: raw,
		baseWebsocketConnection: NewBaseWebsocketConnection(
			uuid.NewString(),
			req,
			ctx,
			cancel,
		),
	}

	conn.parent = conn
	return conn
}

func (c *FooWebsocketConnection) Send(ctx context.Context, data []byte, handlers ...connection.MessageSentHandler) error {
	if c.Closed() {
		return connection.ErrConnectionClosed
	}

	select {
	case <-ctx.Done():
		return ctx.Err()
	default:
	}

	if err := c.raw.WriteMessage(foo.TextMessage, data); err != nil {
		return err
	}

	for _, handler := range handlers {
		if err := handler(); err != nil {
			return err
		}
	}
	return nil
}

func (c *FooWebsocketConnection) ReadMessage(ctx context.Context) ([]byte, error) {
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	default:
	}

	_, msg, err := c.raw.ReadMessage()
	return msg, err
}

func (c *FooWebsocketConnection) GetRemoteAddr() string {
	return c.raw.RemoteAddr().String()
}

func (c *FooWebsocketConnection) Close() error {
	if !c.tryClose() {
		return nil
	}
	if c.cancel != nil {
		c.cancel()
	}
	return c.raw.Close()
}
```

Notes:

- Do not call `Close()` directly in backend lifecycle code when you intend to
  close a live connection. Use `server.CloseConnection(conn)`.
- `BindJson` and `SendJson` are already implemented by `baseWebsocketConnection`.
- `ReadMessage` must return only payload bytes and should surface close,
  timeout, context, and network errors instead of swallowing them.
- If the backend connection is not safe for concurrent writes, use a write
  buffer or write loop like the Gorilla implementation.

### 2.2 Server type

Responsibilities:

- Embed `baseWebsocketServer`.
- Hold backend-specific fields such as an upgrader, accept options, or router.
- Initialize the base with `NewDefaultBaseWebsocketServerImplementation()`.
- Implement `AcceptWebsocket(w http.ResponseWriter, r *http.Request)`:
  - Perform the WebSocket upgrade/handshake.
  - Mark `acceptedAtLeastOneConnection` true after a successful accept.
  - Apply `ReadLimit()` to the raw connection if the backend supports it.
  - Wrap the raw connection in your connection type.
  - Wrap the connection in `safeClosableWebsocketConnection` when you need the
    same single-close behavior used by Gorilla.
  - Run active pipeline handlers.
  - Start the command-processing loop.
- Implement `FiberHandler() fiber.Handler`. A `net/http`-based backend can use
  Fiber's official adaptor as shown below.

Typical pattern:

```go
type FooWebsocketServer struct {
	baseWebsocketServer
	upgrader foo.Upgrader
}

func MakeFooWebsocketServer(upgrader foo.Upgrader) WebsocketServer {
	return &FooWebsocketServer{
		baseWebsocketServer: NewDefaultBaseWebsocketServerImplementation(),
		upgrader:            upgrader,
	}
}

func (s *FooWebsocketServer) AcceptWebsocket(w http.ResponseWriter, r *http.Request) {
	rawConn, err := s.upgrader.Upgrade(w, r, nil)
	if err != nil {
		log.Println("upgrade error:", err)
		return
	}

	s.acceptedAtLeastOneConnection.Store(true)

	if limiter, ok := any(rawConn).(interface{ SetReadLimit(int64) }); ok {
		limiter.SetReadLimit(s.ReadLimit())
	}

	originalCon := NewFooWebsocketConnection(rawConn, r)

	var safeCon *safeClosableWebsocketConnection
	safeCon = &safeClosableWebsocketConnection{
		WebsocketConnection: originalCon,
		singleClosable: sync.OnceFunc(func() {
			s.closeConnection(safeCon)
		}),
	}

	for _, f := range s.channelActivePipeline {
		f(safeCon)
	}

	if safeCon.Closed() {
		log.Println("connection closed by an active handler or user")
		return
	}

	go s.IncomingCommand(safeCon)
}

func (s *FooWebsocketServer) FiberHandler() fiber.Handler {
	return adaptor.HTTPHandlerWithContext(http.HandlerFunc(s.AcceptWebsocket))
}
```

If your connection has a write loop, use the Gorilla pattern: start the write
loop in one goroutine, start `IncomingCommand` in another, and defer
`CloseConnection` from the goroutine that owns the loop.

Because the server embeds `baseWebsocketServer`, it gets these methods from the
base implementation:

- `AddCommandHandler`, `RemoveCommandHandler`
- `AddOnActiveHandler`, `AddOnInactiveHandler`
- `AddUser`, `RemoveUser`, `GetUserConnection`, `GetUserConnections`
- `GetChannel`
- `CloseConnection`
- `Broadcast`, `BroadcastJson`
- `SetReadLimit`, `ReadLimit`
- `AtomicCounters`, `AtomicCounter`, `Metrics`

---

## 3. Pipelines and Lifecycle

### Active pipeline

Active handlers run immediately after a connection is accepted:

```go
type ActiveHandler func(connection.WebsocketConnection)
type ActivePipeline []ActiveHandler
```

Common active handler work:

- Authenticate the user.
- Parse user or channel data from headers, query params, or a token.
- Store values with `conn.Local(key, value)`.
- Add the connection with `server.AddUser(id, channel, conn)`.
- Close the connection through `server.CloseConnection(conn)` when validation fails.

Register active handlers before the first connection is accepted:

```go
server.AddOnActiveHandler(func(c connection.WebsocketConnection) {
	server.AddUser("user-id", "channel-name", c)
})
```

Adding active or inactive handlers after `acceptedAtLeastOneConnection` is true
panics.

### Inactive pipeline

Inactive handlers run inside `baseWebsocketServer.CloseConnection`:

```go
type InActiveHandler func(connection.WebsocketConnection)
type InActivePipeline []InActiveHandler
```

Use inactive handlers to remove users from channels, update presence, notify
other services, and release per-connection resources.

```go
server.AddOnInactiveHandler(func(c connection.WebsocketConnection) {
	userID, _ := c.Local("user_id", nil)
	channel, _ := c.Local("channel", nil)
	server.RemoveUser(c, userID.(string), channel.(string))
})
```

Again: do not call `Close()` directly. Always close through
`server.CloseConnection(conn)`.

---

## 4. User and Channel Management

The base server provides:

```go
func (base *baseWebsocketServer) AddUser(id string, channel string, con connection.WebsocketConnection)
func (base *baseWebsocketServer) RemoveUser(con connection.WebsocketConnection, id string, channel string)
func (base *baseWebsocketServer) GetUserConnection(id string, channel string) ([]connection.WebsocketConnection, bool)
func (base *baseWebsocketServer) GetUserConnections(id string) (*UserConnections, bool)
func (base *baseWebsocketServer) GetChannel(channel string) (*ChannelHub, bool)
```

`AddUser` requires `con.Id()` to be non-empty. Use
`NewBaseWebsocketConnection` with a generated ID, as the Gorilla backend does.

Internally, users are tracked by `ConnectedUsers`:

```go
type ConnectedUsers = _map.SynchronizedMap[string, *UserConnections]
```

Each `UserConnections` maps channels to connection slices. Channel membership
is also tracked by `ChannelHub`. Backend implementors usually do not need to
touch those structures directly; call the public methods from active and
inactive handlers.

---

## 5. JSON, Commands, and Errors

Incoming messages are expected to match:

```go
type IncomingMessage struct {
	Command string `json:"command"`
	Data    JsonAny `json:"data"`
}
```

The default `IncomingCommand` loop:

- Reads JSON using `gcon.BindJson(gcon.Context(), &msg)`.
- Increments receive/error metrics.
- Continues on JSON decode errors.
- Returns on close/read errors.
- Dispatches known commands to registered handlers.
- Logs unknown commands and increments the unknown-command metric.

`baseWebsocketConnection.BindJson` uses your `ReadMessage` and `encoding/json`:

```go
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
```

If you need to classify JSON errors, use the existing `isJsonError` helper.

Backend rules:

- Propagate close, network, and context errors.
- Do not wrap every read error in a generic error, because `IncomingCommand`
  checks for `connection.ErrConnectionClosed`.
- Avoid a `ReadMessage` implementation that can block forever after context
  cancellation or socket closure.

---

## 6. Read Limits and Metrics

`NewDefaultBaseWebsocketServerImplementation` sets a default read limit of
10 MiB:

```go
readLimit: 1024 * 1024 * 10
```

Backend servers should apply `ReadLimit()` to their concrete library when it
supports read limits. Callers can adjust it with:

```go
server.SetReadLimit(1024 * 1024)
```

The base server also exposes:

```go
server.AtomicCounters()
server.AtomicCounter(server.TotalMessagesReceived)
server.Metrics()
```

Do not create a separate metrics system for a backend unless the library needs
extra backend-specific counters.

---

## 7. Checklist for New Backends

Before submitting a PR:

- Add a connection type that embeds `baseWebsocketConnection`.
- Initialize the base connection with `NewBaseWebsocketConnection`.
- Generate a non-empty connection ID.
- Set `conn.parent = conn`.
- Implement `Send(ctx, message, handlers...)`.
- Implement `ReadMessage(ctx)`.
- Implement `Close()` and use `tryClose()`.
- Implement `GetRemoteAddr()`.
- Implement or intentionally omit a backend-specific `WithContext`.
- Add a server type that embeds `baseWebsocketServer`.
- Initialize the base server with `NewDefaultBaseWebsocketServerImplementation`.
- Implement `AcceptWebsocket(w, r)`.
- Implement `FiberHandler()` and verify the real upgrade through a Fiber
  listener.
- Mark `acceptedAtLeastOneConnection` true only after a successful accept.
- Apply `ReadLimit()` if the backend supports it.
- Run active pipeline handlers before starting command processing.
- Close through `server.CloseConnection(conn)`, not `conn.Close()`.
- Use `RemoveUser(con, id, channel)` in inactive cleanup.
- Add tests or examples for connection setup, command dispatch, user cleanup,
  and any backend-specific write-loop behavior.

---

## 8. PR Expectations

When opening a PR for a new backend:

- Document the WebSocket library, version, and link.
- Include a short example showing how to construct the server, register
  handlers, and bind it to an HTTP route.
- Explain library-specific behavior such as message types, ping/pong handling,
  close frames, write concurrency, buffering, and read limits.
- Note whether the backend uses a write loop and how message-sent handlers are
  executed.

If you follow this guide, the backend should fit into the existing server logic
without changing higher-level application code.
