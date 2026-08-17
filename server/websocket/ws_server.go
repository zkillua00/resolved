package server

import (
	"encoding/json"
	"errors"
	"log"
	"net/http"
	"slices"
	"sync/atomic"

	"github.com/gofiber/fiber/v3"

	"resolved-server/websocket/connection"
	"resolved-server/websocket/counters"
	_map "resolved-server/websocket/map"
	"resolved-server/websocket/mutex"
)

type IncomingMessage struct {
	Command string  `json:"command"`
	Data    JsonAny `json:"data"`
}

type OutgoingMessage JsonObject

type JsonObject = map[string]any
type JsonAny = any
type InActiveHandler func(connection.WebsocketConnection)
type ActiveHandler func(connection.WebsocketConnection)
type InActivePipeline []InActiveHandler
type ActivePipeline []ActiveHandler

type UserConnections struct {
	mu    mutex.RWMutexWrapper
	conns map[string][]connection.WebsocketConnection
}

func makeUserConnections() *UserConnections {
	return &UserConnections{
		conns: make(map[string][]connection.WebsocketConnection),
		mu:    mutex.MakeRWMutex(),
	}
}

func (ucons *UserConnections) Range(callback func(channel string, con []connection.WebsocketConnection) bool) {
	ucons.mu.RLock()
	defer ucons.mu.RUnlock()
	for channel, con := range ucons.conns {
		if !callback(channel, con) {
			break
		}
	}
}

func (ucons *UserConnections) Store(channel string, con connection.WebsocketConnection) {
	ucons.mu.Lock()
	defer ucons.mu.Unlock()
	ucons.conns[channel] = append(ucons.conns[channel], con)
}

func (ucons *UserConnections) Contains(channel string, con connection.WebsocketConnection) bool {
	ucons.mu.RLock()
	defer ucons.mu.RUnlock()
	sn, ok := ucons.conns[channel]
	if !ok {
		return false
	}
	return slices.Contains(sn, con)
}

func (ucons *UserConnections) Load(channel string) ([]connection.WebsocketConnection, bool) {
	ucons.mu.RLock()
	defer ucons.mu.RUnlock()
	con, ok := ucons.conns[channel]
	return con, ok
}

func (ucons *UserConnections) Snapshot() map[string][]connection.WebsocketConnection {
	ucons.mu.RLock()
	defer ucons.mu.RUnlock()

	// Deep copy
	out := make(map[string][]connection.WebsocketConnection, len(ucons.conns))
	for k, v := range ucons.conns {
		out[k] = slices.Clone(v) // Requires Go 1.21+
	}
	return out
}

func (ucons *UserConnections) Len() int {
	ucons.mu.RLock()
	defer ucons.mu.RUnlock()
	return len(ucons.conns)
}

type ConnectedUsers = _map.SynchronizedMap[string, *UserConnections]

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

type Metrics map[string]uint64

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

func NewDefaultBaseWebsocketServerImplementation() baseWebsocketServer {
	return baseWebsocketServer{
		commandHandlers:              make(map[string]func(connection.WebsocketConnection, JsonAny)),
		channelActivePipeline:        ActivePipeline{},
		channelInactivePipeline:      InActivePipeline{},
		connectedUsers:               ConnectedUsers{},
		acceptedAtLeastOneConnection: atomic.Bool{},
		readLimit:                    1024 * 1024 * 10, // 10mb
		atomicCounters:               counters.NewAtomicCounters(),
	}
}

type safeClosableWebsocketConnection struct {
	connection.WebsocketConnection
	singleClosable func()
}

func (s *safeClosableWebsocketConnection) Close() error {
	return s.WebsocketConnection.(connection.Closable).Close()
}

func (b *baseWebsocketServer) AtomicCounters() counters.AtomicCounters {
	return b.atomicCounters
}

func (b *baseWebsocketServer) AtomicCounter(field string) counters.AtomicCounter {
	return counters.NewAtomicCounter(field, b.atomicCounters)
}

func (b *baseWebsocketServer) Metrics() Metrics {
	m := make(Metrics)
	for _, key := range metricKeys {
		val, ok := b.atomicCounters.Get(key)
		if ok {
			m[key] = uint64(val)
		} else {
			m[key] = 0
		}
	}
	return m
}

func isJsonError(err error) bool {
	var (
		syntaxError           *json.SyntaxError
		unmarshalTypeError    *json.UnmarshalTypeError
		invalidUnmarshalError *json.InvalidUnmarshalError
		unsupportedValue      *json.UnsupportedValueError
		unsupportedType       *json.UnsupportedTypeError
	)

	return errors.As(err, &syntaxError) ||
		errors.As(err, &unmarshalTypeError) ||
		errors.As(err, &invalidUnmarshalError) ||
		errors.As(err, &unsupportedValue) ||
		errors.As(err, &unsupportedType)
}

func defaultAppendPipeline[HT ActiveHandler | InActiveHandler](base *baseWebsocketServer, handler HT) {
	if base.acceptedAtLeastOneConnection.Load() {
		panic("cannot add handler after accepting at least one connection")
	}

	if v, ok := any(handler).(ActiveHandler); ok {
		base.channelActivePipeline = append(base.channelActivePipeline, v)
	} else if v, ok := any(handler).(InActiveHandler); ok {
		base.channelInactivePipeline = append(base.channelInactivePipeline, v)
	}
}

func (base *baseWebsocketServer) AddOnActiveHandler(f ActiveHandler) {
	defaultAppendPipeline(base, f)
}

func (base *baseWebsocketServer) AddOnInactiveHandler(f InActiveHandler) {
	defaultAppendPipeline(base, f)
}

func defaultAddCommandHandler(base *baseWebsocketServer, command string, handler func(connection.WebsocketConnection, JsonAny)) {
	if base.acceptedAtLeastOneConnection.Load() {
		panic("cannot add handler after accepting at least one connection")
	}
	if base.commandHandlers == nil {
		base.commandHandlers = make(map[string]func(connection.WebsocketConnection, JsonAny))
	}
	base.commandHandlers[command] = handler
}

func (base *baseWebsocketServer) AddCommandHandler(command string, handler func(connection.WebsocketConnection, JsonAny)) {
	defaultAddCommandHandler(base, command, handler)
}

func defaultRemoveCommandHandler(base *baseWebsocketServer, command string) {
	if base.acceptedAtLeastOneConnection.Load() {
		panic("cannot remove handler after accepting at least one connection")
	}
	if base.commandHandlers == nil {
		return
	}
	delete(base.commandHandlers, command)
}

func (base *baseWebsocketServer) RemoveCommandHandler(command string) {
	defaultRemoveCommandHandler(base, command)
}

func (base *baseWebsocketServer) GetChannel(channel string) (*ChannelHub, bool) {
	return base.channelUsers.Load(channel)
}

func defaultAddUser(base *baseWebsocketServer, id string, channel string, con connection.WebsocketConnection) {
	if con.Id() == "" {
		panic("connection must have an id")
	}

	deadmanCount := 0
	explosionLimit := 10

	for {
		userConnections, _ := base.connectedUsers.LoadOrStore(id, makeUserConnections())

		userConnections.mu.Lock()
		if actual, ok := base.connectedUsers.Load(id); !ok || actual != userConnections {
			userConnections.mu.Unlock()
			if deadmanCount > explosionLimit {
				panic("tried to allocate the user connections way too many times.")
			}
			deadmanCount++
			continue
		}

		if slices.Contains(userConnections.conns[channel], con) {
			userConnections.mu.Unlock()
			return
		}

		userConnections.conns[channel] = append(userConnections.conns[channel], con)
		userConnections.mu.Unlock()
		break
	}

	handleJoin(base, channel, con)
}

func (base *baseWebsocketServer) AddUser(id string, channel string, con connection.WebsocketConnection) {
	defaultAddUser(base, id, channel, con)
}
func removeUserConnection(base *baseWebsocketServer, con connection.WebsocketConnection, id string, channel string) {
	val, ok := base.connectedUsers.Load(id)
	if !ok {
		return
	}

	userConns := val

	muRef := &userConns.mu
	muRef.Lock()
	defer muRef.Unlock()

	oldSlice := userConns.conns[channel]

	newSlice := slices.DeleteFunc(oldSlice, func(c connection.WebsocketConnection) bool {
		return c == con
	})

	userConns.conns[channel] = newSlice

	if len(userConns.conns[channel]) == 0 {
		delete(userConns.conns, channel)
	}
	if len(userConns.conns) == 0 {
		base.connectedUsers.Delete(id)
	}
}

func removeUserFromChannel(base *baseWebsocketServer, con connection.WebsocketConnection, channel string) {
	hub, ok := base.channelUsers.Load(channel)
	if !ok {
		log.Println("channel not found:", channel)
		return
	}
	hub.LeaveAndDeleteIfEmpty(con, func(current *ChannelHub) bool {
		return base.channelUsers.CompareAndDelete(channel, current)
	})
}

func defaultRemoveUser(base *baseWebsocketServer, con connection.WebsocketConnection, id string, channel string) {
	removeUserConnection(base, con, id, channel)
	removeUserFromChannel(base, con, channel)
}

func (base *baseWebsocketServer) RemoveUser(con connection.WebsocketConnection, id string, channel string) {
	defaultRemoveUser(base, con, id, channel)
}

func defaultGetUser(base *baseWebsocketServer, id string, channel string) ([]connection.WebsocketConnection, bool) {
	ucons, ok := base.connectedUsers.Load(id)
	if !ok {
		return nil, false
	}
	return ucons.Load(channel)
}

func (base *baseWebsocketServer) GetUserConnection(id string, channel string) ([]connection.WebsocketConnection, bool) {
	return defaultGetUser(base, id, channel)
}

func (base *baseWebsocketServer) GetUserConnections(id string) (*UserConnections, bool) {
	return base.connectedUsers.Load(id)
}

func (base *baseWebsocketServer) closeConnection(con connection.WebsocketConnection) {
	if con.Closed() {
		return
	}

	defer func() {
		if err := con.(connection.Closable).Close(); err != nil { // should panic if not closable. every connection type must be closable. it is not embedded to the websocket connection interface to hide the requirement from the user. the user must call server.CloseConnection(connection) instead of connection.Close()
			log.Println("close error:", err)
		}
	}()

	for _, f := range base.channelInactivePipeline {
		f(con)
	}
}

func (base *baseWebsocketServer) CloseConnection(gcon connection.WebsocketConnection) {
	if v, ok := gcon.(*safeClosableWebsocketConnection); ok && v.singleClosable != nil {
		v.singleClosable()
	} else {
		base.closeConnection(gcon)
	}
}

func (base *baseWebsocketServer) broadcastToAllChannels(message []byte) {
	seen := make(map[string]struct{})
	base.connectedUsers.Range(func(_ string, uCons *UserConnections) bool {
		uCons.Range(func(_ string, con []connection.WebsocketConnection) bool {
			for _, chSock := range con {
				id := chSock.Id()
				if _, ok := seen[id]; ok {
					continue
				}
				seen[id] = struct{}{}
				_ = chSock.Send(chSock.Context(), message)
			}
			return true
		})
		return true
	})
}

func (base *baseWebsocketServer) broadcastToChannel(channel string, message []byte) {
	users, ok := base.channelUsers.Load(channel)
	if !ok {
		return
	}
	users.Range(func(chSock connection.WebsocketConnection) bool {
		_ = chSock.Send(chSock.Context(), message)
		return true
	})
}

func (base *baseWebsocketServer) IncomingCommand(gcon connection.WebsocketConnection) {
	defer base.CloseConnection(gcon)

	for {
		var msg IncomingMessage
		err := gcon.BindJson(gcon.Context(), &msg)
		base.AtomicCounter(TotalMessagesReceived).Increase(1)
		if err != nil {
			base.AtomicCounter(TotalErronousMessagesReceived).Increase(1)
			if errors.Is(err, connection.ErrConnectionClosed) {
				base.AtomicCounter(TotalConnectionClosedMsgsReceived).Increase(1)
				log.Println("connection closed:", err)
			} else if isJsonError(err) {
				base.AtomicCounter(TotalJsonErrorMessagesReceived).Increase(1)
				log.Println("json error:", err)
				continue
			} else {
				base.AtomicCounter(TotalReadErrorMessagesReceived).Increase(1)
				log.Println("read error:", err)
			}
			return
		}

		handler, ok := base.commandHandlers[msg.Command]
		if ok {
			handler(gcon, msg.Data)
			base.AtomicCounter(TotalHandledCommands).Increase(1)
		} else {
			base.AtomicCounter(TotalUnknownCommands).Increase(1)
			log.Println("unknown command:", msg.Command)
		}
	}
}

func (base *baseWebsocketServer) BroadcastJson(data interface{}, channel *string) {
	body, err := json.Marshal(data)
	if err != nil {
		base.AtomicCounter(TotalFailedToBroadcastJsonMessages).Increase(1)
		log.Println("error marshalling json:", err)
		return
	}
	base.AtomicCounter(TotalBroadcastedJsonMessages).Increase(1)
	base.Broadcast(body, channel)
}

func (base *baseWebsocketServer) Broadcast(message []byte, channel *string) {
	if channel == nil {
		base.broadcastToAllChannels(message)
		base.AtomicCounter(TotalBroadcastedMsgsToAllChannels).Increase(1)
		return
	}
	base.broadcastToChannel(*channel, message)
	base.AtomicCounter(TotalBroadcastedMsgsToChannel).Increase(1)
}

func (base *baseWebsocketServer) ReadLimit() int64 {
	return base.readLimit
}

func (base *baseWebsocketServer) SetReadLimit(limit int64) {
	base.readLimit = limit
}
