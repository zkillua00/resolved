package server

import (
	"fmt"
	"runtime/debug"
	"slices"
	"sync/atomic"
	"time"

	"resolved-server/websocket/connection"
	"resolved-server/websocket/mutex"
)

const panicIn = 5 * time.Second
const ShouldPanic = false

type ChannelHub struct {
	mu          mutex.RWMutexWrapper
	connections []connection.WebsocketConnection

	gen       atomic.Uint64
	holdStack atomic.Value
}

func (c *ChannelHub) startHoldWatchdog(d time.Duration) (cancel func()) {
	if !ShouldPanic {
		return func() {}
	}

	id := c.gen.Add(1)
	c.holdStack.Store(debug.Stack())

	t := time.AfterFunc(d, func() {
		if c.gen.Load() == id {
			if v := c.holdStack.Load(); v != nil {
				panic(fmt.Errorf("lock held > %s\nholder stack:\n%s\npanic stack:\n%s",
					d, string(v.([]byte)), string(debug.Stack())))
			}
			panic(fmt.Errorf("lock held > %s\npanic stack:\n%s", d, string(debug.Stack())))
		}
	})

	return func() {
		c.gen.Add(1)
		t.Stop()
	}
}

func (c *ChannelHub) Join(con connection.WebsocketConnection) {
	c.mu.Lock()
	cancel := c.startHoldWatchdog(panicIn)
	defer func() {
		cancel()
		c.mu.Unlock()
	}()

	c.connections = append(c.connections, con)
}

func (c *ChannelHub) Leave(con connection.WebsocketConnection) {
	c.mu.Lock()
	cancel := c.startHoldWatchdog(panicIn)
	defer func() {
		cancel()
		c.mu.Unlock()
	}()

	c.connections = slices.DeleteFunc(c.connections, func(existing connection.WebsocketConnection) bool {
		return existing == con
	})
}

func (c *ChannelHub) LeaveAndDeleteIfEmpty(con connection.WebsocketConnection, deleteIfCurrent func(*ChannelHub) bool) {
	c.mu.Lock()
	cancel := c.startHoldWatchdog(panicIn)
	defer func() {
		cancel()
		c.mu.Unlock()
	}()

	c.connections = slices.DeleteFunc(c.connections, func(existing connection.WebsocketConnection) bool {
		return existing == con
	})

	if len(c.connections) == 0 {
		deleteIfCurrent(c)
	}
}

func (c *ChannelHub) Snapshot() []connection.WebsocketConnection {
	c.mu.RLock()
	cancel := c.startHoldWatchdog(panicIn)
	defer func() {
		cancel()
		c.mu.RUnlock()
	}()

	return slices.Clone(c.connections)
}

func (c *ChannelHub) Range(callback func(con connection.WebsocketConnection) bool) {
	c.mu.RLock()
	cancel := c.startHoldWatchdog(panicIn)
	defer func() {
		cancel()
		c.mu.RUnlock()
	}()

	for _, con := range c.connections {
		if !callback(con) {
			break
		}
	}
}

func (c *ChannelHub) Len() int {
	c.mu.RLock()
	cancel := c.startHoldWatchdog(panicIn)
	defer func() {
		cancel()
		c.mu.RUnlock()
	}()

	return len(c.connections)
}

func (c *ChannelHub) IsEmpty() bool {
	return c.Len() == 0
}

func handleJoin(base *baseWebsocketServer, channelID string, con connection.WebsocketConnection) {
	for {
		hub, ok := base.channelUsers.Load(channelID)
		if !ok {
			hub = &ChannelHub{
				connections: make([]connection.WebsocketConnection, 0),
				mu:          mutex.MakeRWMutex(),
			}
			hub, _ = base.channelUsers.LoadOrStore(channelID, hub)
		}

		hub.Join(con)

		loadedHub, ok := base.channelUsers.Load(channelID)
		if ok && loadedHub == hub {
			return
		}
		hub.Leave(con)
	}
}
