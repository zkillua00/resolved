package server

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"resolved-server/websocket/connection"
	"resolved-server/websocket/mutex"
)

type stressConnection struct {
	id      string
	headers http.Header
	locals  sync.Map

	ctx    context.Context
	cancel context.CancelFunc

	incoming chan []byte
	sentMu   sync.Mutex
	sent     [][]byte
	closed   atomic.Bool
}

func newStressConnection(id string, buffer int) *stressConnection {
	ctx, cancel := context.WithCancel(context.Background())
	return &stressConnection{
		id:       id,
		headers:  http.Header{},
		ctx:      ctx,
		cancel:   cancel,
		incoming: make(chan []byte, buffer),
	}
}

func (s *stressConnection) Id() string {
	return s.id
}

func (s *stressConnection) Send(ctx context.Context, message []byte, handlers ...connection.MessageSentHandler) error {
	if s.Closed() {
		return connection.ErrConnectionClosed
	}

	select {
	case <-ctx.Done():
		return ctx.Err()
	default:
	}

	s.sentMu.Lock()
	s.sent = append(s.sent, append([]byte(nil), message...))
	s.sentMu.Unlock()

	for _, handler := range handlers {
		if err := handler(); err != nil {
			return err
		}
	}
	return nil
}

func (s *stressConnection) SendJson(ctx context.Context, data any, handlers ...connection.MessageSentHandler) error {
	body, err := json.Marshal(data)
	if err != nil {
		return err
	}
	return s.Send(ctx, body, handlers...)
}

func (s *stressConnection) ReadMessage(ctx context.Context) ([]byte, error) {
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-s.ctx.Done():
		return nil, connection.ErrConnectionClosed
	case msg, ok := <-s.incoming:
		if !ok {
			return nil, connection.ErrConnectionClosed
		}
		return msg, nil
	}
}

func (s *stressConnection) BindJson(ctx context.Context, data any) error {
	body, err := s.ReadMessage(ctx)
	if err != nil {
		return err
	}
	return json.Unmarshal(body, data)
}

func (s *stressConnection) Local(key string, data any) (any, bool) {
	if data == nil {
		return s.locals.Load(key)
	}
	s.locals.Store(key, data)
	return data, true
}

func (s *stressConnection) Locals() map[string]any {
	out := make(map[string]any)
	s.locals.Range(func(key, value any) bool {
		out[key.(string)] = value
		return true
	})
	return out
}

func (s *stressConnection) Header(key string) (string, bool) {
	value := s.headers.Get(key)
	return value, value != ""
}

func (s *stressConnection) Closed() bool {
	return s.closed.Load()
}

func (s *stressConnection) GetRemoteAddr() string {
	return "stress-test"
}

func (s *stressConnection) Context() context.Context {
	return s.ctx
}

func (s *stressConnection) Close() error {
	if !s.closed.CompareAndSwap(false, true) {
		return nil
	}
	s.cancel()
	return nil
}

func (s *stressConnection) enqueue(t *testing.T, msg IncomingMessage) {
	t.Helper()
	body, err := json.Marshal(msg)
	if err != nil {
		t.Fatalf("marshal incoming message: %v", err)
	}
	s.incoming <- body
}

func (s *stressConnection) sentCount() int {
	s.sentMu.Lock()
	defer s.sentMu.Unlock()
	return len(s.sent)
}

type benchmarkConnection struct {
	id      string
	headers http.Header
	locals  sync.Map

	ctx    context.Context
	cancel context.CancelFunc

	incoming    chan []byte
	writeBuffer chan benchmarkWrite
	closed      atomic.Bool
	sent        atomic.Int64
	enqueued    atomic.Int64
	dropped     atomic.Int64
	writerDone  chan struct{}
	writerOnce  sync.Once
}

type benchmarkWrite struct {
	ctx      context.Context
	handlers []connection.MessageSentHandler
}

func newBenchmarkConnection(id string, buffer int) *benchmarkConnection {
	ctx, cancel := context.WithCancel(context.Background())
	return &benchmarkConnection{
		id:          id,
		headers:     http.Header{},
		ctx:         ctx,
		cancel:      cancel,
		incoming:    make(chan []byte, buffer),
		writeBuffer: make(chan benchmarkWrite, 512),
		writerDone:  make(chan struct{}),
	}
}

func (b *benchmarkConnection) Id() string {
	return b.id
}

func (b *benchmarkConnection) Send(ctx context.Context, message []byte, handlers ...connection.MessageSentHandler) error {
	if b.Closed() {
		return connection.ErrConnectionClosed
	}

	select {
	case b.writeBuffer <- benchmarkWrite{ctx: ctx, handlers: handlers}:
		b.enqueued.Add(1)
		return nil
	case <-b.ctx.Done():
		return connection.ErrConnectionClosed
	case <-ctx.Done():
		return ctx.Err()
	default:
		b.dropped.Add(1)
		return connection.ErrWriteBufferFull
	}
}

func (b *benchmarkConnection) SendJson(ctx context.Context, data any, handlers ...connection.MessageSentHandler) error {
	body, err := json.Marshal(data)
	if err != nil {
		return err
	}
	return b.Send(ctx, body, handlers...)
}

func (b *benchmarkConnection) ReadMessage(ctx context.Context) ([]byte, error) {
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-b.ctx.Done():
		return nil, connection.ErrConnectionClosed
	case msg, ok := <-b.incoming:
		if !ok {
			return nil, connection.ErrConnectionClosed
		}
		return msg, nil
	}
}

func (b *benchmarkConnection) BindJson(ctx context.Context, data any) error {
	body, err := b.ReadMessage(ctx)
	if err != nil {
		return err
	}
	return json.Unmarshal(body, data)
}

func (b *benchmarkConnection) Local(key string, data any) (any, bool) {
	if data == nil {
		return b.locals.Load(key)
	}
	b.locals.Store(key, data)
	return data, true
}

func (b *benchmarkConnection) Locals() map[string]any {
	out := make(map[string]any)
	b.locals.Range(func(key, value any) bool {
		out[key.(string)] = value
		return true
	})
	return out
}

func (b *benchmarkConnection) Header(key string) (string, bool) {
	value := b.headers.Get(key)
	return value, value != ""
}

func (b *benchmarkConnection) Closed() bool {
	return b.closed.Load()
}

func (b *benchmarkConnection) GetRemoteAddr() string {
	return "benchmark"
}

func (b *benchmarkConnection) Context() context.Context {
	return b.ctx
}

func (b *benchmarkConnection) Close() error {
	if !b.closed.CompareAndSwap(false, true) {
		return nil
	}
	b.cancel()
	return nil
}

func (b *benchmarkConnection) StartWriter() {
	b.writerOnce.Do(func() {
		go b.writeLoop()
	})
}

func (b *benchmarkConnection) writeLoop() {
	defer close(b.writerDone)

	for {
		select {
		case <-b.ctx.Done():
			return
		case write := <-b.writeBuffer:
			select {
			case <-write.ctx.Done():
				continue
			default:
			}

			b.sent.Add(1)
			for _, handler := range write.handlers {
				_ = handler()
			}
		}
	}
}

func (b *benchmarkConnection) WaitWriter() {
	<-b.writerDone
}

func makeStressServer() *baseWebsocketServer {
	base := NewDefaultBaseWebsocketServerImplementation()
	return &base
}

func runConcurrent(total int, fn func(i int)) {
	var wg sync.WaitGroup
	wg.Add(total)
	for i := 0; i < total; i++ {
		i := i
		go func() {
			defer wg.Done()
			fn(i)
		}()
	}
	wg.Wait()
}

func TestStressChannelHubConcurrentJoinLeaveSnapshotRange(t *testing.T) {
	t.Parallel()

	hub := &ChannelHub{
		mu:          mutex.MakeRWMutex(),
		connections: make([]connection.WebsocketConnection, 0),
	}

	const connectionCount = 256
	const readerCount = 64

	conns := make([]*stressConnection, connectionCount)
	for i := range conns {
		conns[i] = newStressConnection(fmt.Sprintf("channel-%d", i), 1)
	}

	runConcurrent(connectionCount, func(i int) {
		hub.Join(conns[i])
	})

	if got := hub.Len(); got != connectionCount {
		t.Fatalf("hub length after joins = %d, want %d", got, connectionCount)
	}

	runConcurrent(readerCount, func(i int) {
		_ = hub.Snapshot()
		seen := 0
		hub.Range(func(con connection.WebsocketConnection) bool {
			if con == nil {
				t.Errorf("nil connection observed in range")
				return false
			}
			seen++
			return seen < connectionCount/2 || i%2 == 0
		})
	})

	runConcurrent(connectionCount, func(i int) {
		hub.Leave(conns[i])
	})

	if got := hub.Len(); got != 0 {
		t.Fatalf("hub length after leaves = %d, want 0", got)
	}
}

func TestStressUserConnectionsConcurrentStoreLoadSnapshot(t *testing.T) {
	t.Parallel()

	users := makeUserConnections()
	const connectionCount = 512
	const channelCount = 16

	runConcurrent(connectionCount, func(i int) {
		users.Store(fmt.Sprintf("channel-%02d", i%channelCount), newStressConnection(fmt.Sprintf("usercon-%d", i), 1))
	})

	if got := users.Len(); got != channelCount {
		t.Fatalf("channel count = %d, want %d", got, channelCount)
	}

	runConcurrent(connectionCount, func(i int) {
		channel := fmt.Sprintf("channel-%02d", i%channelCount)
		if conns, ok := users.Load(channel); !ok || len(conns) == 0 {
			t.Errorf("missing connections for %s", channel)
		}
		_ = users.Snapshot()
	})
}

func TestStressAtomicCountersConcurrentOperations(t *testing.T) {
	t.Parallel()

	base := makeStressServer()
	const workers = 128
	const iterations = 500

	counter := base.AtomicCounter("stress-counter")
	runConcurrent(workers, func(i int) {
		for j := 0; j < iterations; j++ {
			counter.Increase(2)
			counter.Decrease(1)
			base.AtomicCounters().Increase("shared", 1)
		}
	})

	want := int64(workers * iterations)
	if got := counter.Get(); got != want {
		t.Fatalf("wrapped counter = %d, want %d", got, want)
	}
	if got, ok := base.AtomicCounters().Get("shared"); !ok || got != want {
		t.Fatalf("shared counter = %d, %t; want %d, true", got, ok, want)
	}
}

func TestStressBaseServerUserAndChannelManagement(t *testing.T) {
	t.Parallel()

	base := makeStressServer()
	const connectionCount = 512
	const userCount = 64
	const channelCount = 8

	conns := make([]*stressConnection, connectionCount)
	for i := range conns {
		conns[i] = newStressConnection(fmt.Sprintf("server-%d", i), 1)
	}

	runConcurrent(connectionCount, func(i int) {
		base.AddUser(
			fmt.Sprintf("user-%02d", i%userCount),
			fmt.Sprintf("channel-%02d", i%channelCount),
			conns[i],
		)
	})

	if got := base.UserCount(); got != userCount {
		t.Fatalf("user count after adds = %d, want %d", got, userCount)
	}

	runConcurrent(connectionCount, func(i int) {
		user := fmt.Sprintf("user-%02d", i%userCount)
		channel := fmt.Sprintf("channel-%02d", i%channelCount)
		if got, ok := base.GetUserConnection(user, channel); ok && len(got) == 0 {
			t.Errorf("empty connection slice for %s/%s", user, channel)
		}
		if hub, ok := base.GetChannel(channel); ok {
			_ = hub.Snapshot()
		}
	})

	runConcurrent(connectionCount, func(i int) {
		base.RemoveUser(
			conns[i],
			fmt.Sprintf("user-%02d", i%userCount),
			fmt.Sprintf("channel-%02d", i%channelCount),
		)
	})

	if got := base.UserCount(); got != 0 {
		t.Fatalf("user count after removals = %d, want 0", got)
	}
	if got := base.channelUsers.Len(); got != 0 {
		t.Fatalf("channel count after removals = %d, want 0", got)
	}
}

func TestStressCloseConnectionRunsInactiveCleanupOnce(t *testing.T) {
	t.Parallel()

	base := makeStressServer()
	conn := newStressConnection("close-stress", 1)

	var inactiveCalls atomic.Int64
	base.AddOnInactiveHandler(func(c connection.WebsocketConnection) {
		inactiveCalls.Add(1)
		base.RemoveUser(c, "user", "channel")
	})

	safeCon := &safeClosableWebsocketConnection{
		WebsocketConnection: conn,
	}
	safeCon.singleClosable = sync.OnceFunc(func() {
		base.closeConnection(safeCon)
	})

	base.AddUser("user", "channel", safeCon)

	runConcurrent(128, func(i int) {
		base.CloseConnection(safeCon)
	})

	if got := inactiveCalls.Load(); got != 1 {
		t.Fatalf("inactive calls = %d, want 1", got)
	}
	if !conn.Closed() {
		t.Fatalf("connection was not closed")
	}
	if got := base.UserCount(); got != 0 {
		t.Fatalf("user count after close = %d, want 0", got)
	}
}

func TestStressBroadcastWhileMembershipChanges(t *testing.T) {
	t.Parallel()

	base := makeStressServer()
	const connectionCount = 256
	const channelCount = 4

	conns := make([]*stressConnection, connectionCount)
	for i := range conns {
		conns[i] = newStressConnection(fmt.Sprintf("broadcast-%d", i), 1)
		base.AddUser(
			fmt.Sprintf("user-%03d", i),
			fmt.Sprintf("channel-%02d", i%channelCount),
			conns[i],
		)
	}

	var broadcasts sync.WaitGroup
	broadcasts.Add(channelCount + 1)
	for channel := 0; channel < channelCount; channel++ {
		channel := fmt.Sprintf("channel-%02d", channel)
		go func() {
			defer broadcasts.Done()
			for i := 0; i < 100; i++ {
				base.Broadcast([]byte("channel-message"), &channel)
			}
		}()
	}
	go func() {
		defer broadcasts.Done()
		for i := 0; i < 100; i++ {
			base.Broadcast([]byte("all-message"), nil)
		}
	}()

	runConcurrent(connectionCount, func(i int) {
		if i%2 == 0 {
			base.RemoveUser(
				conns[i],
				fmt.Sprintf("user-%03d", i),
				fmt.Sprintf("channel-%02d", i%channelCount),
			)
			base.AddUser(
				fmt.Sprintf("user-%03d", i),
				fmt.Sprintf("channel-%02d", i%channelCount),
				conns[i],
			)
		}
	})

	broadcasts.Wait()

	totalSent := 0
	for _, conn := range conns {
		totalSent += conn.sentCount()
	}
	if totalSent == 0 {
		t.Fatalf("expected broadcasts to reach at least one connection")
	}
}

func TestChannelHubDeleteDoesNotLoseConcurrentJoin(t *testing.T) {
	base := NewDefaultBaseWebsocketServerImplementation()
	oldConn := newStressConnection("old", 1)
	newConn := newStressConnection("new", 1)

	base.AddUser("old-user", "room", oldConn)
	hub, ok := base.channelUsers.Load("room")
	if !ok {
		t.Fatal("expected room hub to exist")
	}

	joinStarted := make(chan struct{})
	joinDone := make(chan struct{})
	hub.LeaveAndDeleteIfEmpty(oldConn, func(current *ChannelHub) bool {
		go func() {
			close(joinStarted)
			base.AddUser("new-user", "room", newConn)
			close(joinDone)
		}()

		<-joinStarted
		return base.channelUsers.CompareAndDelete("room", current)
	})

	select {
	case <-joinDone:
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for concurrent join")
	}

	room, ok := base.GetChannel("room")
	if !ok {
		t.Fatal("room channel disappeared after concurrent join")
	}

	snapshot := room.Snapshot()
	if len(snapshot) != 1 || snapshot[0] != newConn {
		t.Fatalf("room channel connections = %v, want only the new connection", len(snapshot))
	}
}

func TestStressIncomingCommandDispatch(t *testing.T) {
	t.Parallel()

	base := makeStressServer()
	const connectionCount = 64
	const messagesPerConnection = 100

	var handled atomic.Int64
	base.AddCommandHandler("increment", func(c connection.WebsocketConnection, data JsonAny) {
		handled.Add(1)
	})

	var wg sync.WaitGroup
	wg.Add(connectionCount)
	for i := 0; i < connectionCount; i++ {
		conn := newStressConnection(fmt.Sprintf("incoming-%d", i), messagesPerConnection)
		for j := 0; j < messagesPerConnection; j++ {
			conn.enqueue(t, IncomingMessage{Command: "increment", Data: map[string]any{"n": j}})
		}
		go func() {
			defer wg.Done()
			base.IncomingCommand(conn)
		}()
		go func() {
			for {
				if handled.Load() >= connectionCount*messagesPerConnection {
					_ = conn.Close()
					return
				}
				time.Sleep(time.Millisecond)
			}
		}()
	}

	done := make(chan struct{})
	go func() {
		wg.Wait()
		close(done)
	}()

	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatalf("incoming command stress test timed out")
	}

	want := int64(connectionCount * messagesPerConnection)
	if got := handled.Load(); got != want {
		t.Fatalf("handled commands = %d, want %d", got, want)
	}
}

func TestStressServerConnectionManagementE2E(t *testing.T) {
	t.Parallel()

	base := makeStressServer()
	const connectionCount = 96
	const messagesPerConnection = 50
	const channelCount = 6

	var handled atomic.Int64
	var inactive atomic.Int64
	base.AddCommandHandler("echo", func(c connection.WebsocketConnection, data JsonAny) {
		handled.Add(1)
		_ = c.SendJson(c.Context(), OutgoingMessage{"command": "echo", "data": data})
	})
	base.AddOnInactiveHandler(func(c connection.WebsocketConnection) {
		inactive.Add(1)
		user, okUser := c.Local("user", nil)
		channel, okChannel := c.Local("channel", nil)
		if okUser && okChannel {
			base.RemoveUser(c, user.(string), channel.(string))
		}
	})

	conns := make([]*stressConnection, connectionCount)
	var wg sync.WaitGroup
	wg.Add(connectionCount)

	for i := 0; i < connectionCount; i++ {
		conn := newStressConnection(fmt.Sprintf("e2e-%d", i), messagesPerConnection)
		user := fmt.Sprintf("user-%03d", i)
		channel := fmt.Sprintf("channel-%02d", i%channelCount)
		conn.Local("user", user)
		conn.Local("channel", channel)
		base.AddUser(user, channel, conn)
		for j := 0; j < messagesPerConnection; j++ {
			conn.enqueue(t, IncomingMessage{Command: "echo", Data: map[string]any{"sequence": j}})
		}
		conns[i] = conn

		go func() {
			defer wg.Done()
			base.IncomingCommand(conn)
		}()
	}

	if got := base.UserCount(); got != connectionCount {
		t.Fatalf("user count after e2e setup = %d, want %d", got, connectionCount)
	}

	doneSending := make(chan struct{})
	go func() {
		defer close(doneSending)
		for handled.Load() < connectionCount*messagesPerConnection {
			base.Broadcast([]byte("tick"), nil)
			time.Sleep(time.Millisecond)
		}
		for _, conn := range conns {
			base.CloseConnection(conn)
		}
	}()

	done := make(chan struct{})
	go func() {
		wg.Wait()
		close(done)
	}()

	select {
	case <-done:
	case <-time.After(10 * time.Second):
		t.Fatalf("e2e stress test timed out")
	}
	<-doneSending

	wantHandled := int64(connectionCount * messagesPerConnection)
	if got := handled.Load(); got != wantHandled {
		t.Fatalf("handled e2e commands = %d, want %d", got, wantHandled)
	}
	if got := inactive.Load(); got != connectionCount {
		t.Fatalf("inactive calls = %d, want %d", got, connectionCount)
	}
	if got := base.UserCount(); got != 0 {
		t.Fatalf("user count after e2e close = %d, want 0", got)
	}

	for _, conn := range conns {
		if conn.sentCount() == 0 {
			t.Fatalf("connection %s did not receive echo or broadcast messages", conn.Id())
		}
	}
}

func TestStressConnectionLocalAndClose(t *testing.T) {
	t.Parallel()

	conn := newStressConnection("local-close", 1)
	const workers = 128
	const iterations = 100

	runConcurrent(workers, func(i int) {
		for j := 0; j < iterations; j++ {
			key := fmt.Sprintf("k-%03d-%03d", i, j)
			conn.Local(key, j)
			if got, ok := conn.Local(key, nil); !ok || got.(int) != j {
				t.Errorf("local %s = %v, %t; want %d, true", key, got, ok, j)
			}
		}
	})

	runConcurrent(workers, func(i int) {
		if err := conn.Close(); err != nil && !errors.Is(err, connection.ErrConnectionClosed) {
			t.Errorf("close error: %v", err)
		}
	})

	if !conn.Closed() {
		t.Fatalf("connection was not closed")
	}
}

func benchmarkConnectionCounts() []int {
	return []int{1, 16, 64, 256, 1024}
}

func discardBenchmarkLogs() func() {
	out := log.Writer()
	log.SetOutput(io.Discard)
	return func() {
		log.SetOutput(out)
	}
}

func makeBenchmarkConnections(base *baseWebsocketServer, connectionCount int, buffer int) []*benchmarkConnection {
	conns := make([]*benchmarkConnection, connectionCount)
	for i := range conns {
		conn := newBenchmarkConnection(fmt.Sprintf("bench-%d", i), buffer)
		user := fmt.Sprintf("user-%06d", i)
		channel := fmt.Sprintf("channel-%02d", i%16)
		conn.Local("user", user)
		conn.Local("channel", channel)
		base.AddUser(user, channel, conn)
		conn.StartWriter()
		conns[i] = conn
	}
	return conns
}

func makeBenchmarkUserConnections(base *baseWebsocketServer, userCount int, connsPerUser int, buffer int) []*benchmarkConnection {
	conns := make([]*benchmarkConnection, 0, userCount*connsPerUser)
	for userIndex := 0; userIndex < userCount; userIndex++ {
		user := fmt.Sprintf("growth-user-%06d", userIndex)
		channel := fmt.Sprintf("growth-channel-%02d", userIndex%16)
		for connIndex := 0; connIndex < connsPerUser; connIndex++ {
			conn := newBenchmarkConnection(fmt.Sprintf("%s-conn-%06d", user, connIndex), buffer)
			conn.Local("user", user)
			conn.Local("channel", channel)
			base.AddUser(user, channel, conn)
			conn.StartWriter()
			conns = append(conns, conn)
		}
	}
	return conns
}

func closeBenchmarkConnections(base *baseWebsocketServer, conns []*benchmarkConnection) {
	for _, conn := range conns {
		base.CloseConnection(conn)
	}
	for _, conn := range conns {
		conn.WaitWriter()
	}
}

func benchmarkConnectionWriteStats(conns []*benchmarkConnection) (enqueued int64, dropped int64, drained int64) {
	for _, conn := range conns {
		enqueued += conn.enqueued.Load()
		dropped += conn.dropped.Load()
		drained += conn.sent.Load()
	}
	return enqueued, dropped, drained
}

func waitForBenchmarkWrites(b *testing.B, conns []*benchmarkConnection) {
	b.Helper()

	deadline := time.After(30 * time.Second)
	for {
		enqueued, _, drained := benchmarkConnectionWriteStats(conns)
		if drained >= enqueued {
			return
		}

		select {
		case <-deadline:
			b.Fatalf("timed out waiting for benchmark writes to drain: enqueued=%d drained=%d", enqueued, drained)
		default:
			time.Sleep(time.Microsecond)
		}
	}
}

func waitForBenchmarkCount(b *testing.B, counter *atomic.Int64, want int64) {
	b.Helper()

	deadline := time.After(30 * time.Second)
	for counter.Load() < want {
		select {
		case <-deadline:
			b.Fatalf("timed out waiting for counter: got %d, want %d", counter.Load(), want)
		default:
			time.Sleep(time.Microsecond)
		}
	}
}

func BenchmarkStressFanInIncreasingConnections(b *testing.B) {
	defer discardBenchmarkLogs()()

	body, err := json.Marshal(IncomingMessage{Command: "bench", Data: JsonObject{"ok": true}})
	if err != nil {
		b.Fatalf("marshal benchmark message: %v", err)
	}

	for _, connectionCount := range benchmarkConnectionCounts() {
		b.Run(fmt.Sprintf("connections=%d", connectionCount), func(b *testing.B) {
			base := makeStressServer()
			var handled atomic.Int64
			base.AddCommandHandler("bench", func(c connection.WebsocketConnection, data JsonAny) {
				handled.Add(1)
			})

			messagesPerConnection := max(1, (b.N+connectionCount-1)/connectionCount)
			totalMessages := messagesPerConnection * connectionCount
			conns := makeBenchmarkConnections(base, connectionCount, messagesPerConnection+1)

			var wg sync.WaitGroup
			wg.Add(connectionCount)
			for _, conn := range conns {
				conn := conn
				go func() {
					defer wg.Done()
					base.IncomingCommand(conn)
				}()
			}

			b.ReportAllocs()
			b.ResetTimer()
			runConcurrent(connectionCount, func(i int) {
				for j := 0; j < messagesPerConnection; j++ {
					conns[i].incoming <- body
				}
			})
			waitForBenchmarkCount(b, &handled, int64(totalMessages))
			waitForBenchmarkWrites(b, conns)
			b.StopTimer()

			closeBenchmarkConnections(base, conns)
			wg.Wait()

			b.ReportMetric(float64(totalMessages), "messages")
			b.ReportMetric(float64(totalMessages)/b.Elapsed().Seconds(), "messages/sec")
		})
	}
}

func BenchmarkStressFanOutBroadcastIncreasingConnections(b *testing.B) {
	for _, connectionCount := range benchmarkConnectionCounts() {
		b.Run(fmt.Sprintf("connections=%d", connectionCount), func(b *testing.B) {
			base := makeStressServer()
			conns := makeBenchmarkConnections(base, connectionCount, 1)
			message := []byte("fanout")
			expectedSends := int64(b.N * connectionCount)

			b.ReportAllocs()
			b.ResetTimer()
			for i := 0; i < b.N; i++ {
				base.Broadcast(message, nil)
			}
			waitForBenchmarkWrites(b, conns)
			b.StopTimer()

			closeBenchmarkConnections(base, conns)
			enqueued, dropped, drained := benchmarkConnectionWriteStats(conns)
			if enqueued+drained == 0 {
				b.Fatalf("fan-out did not enqueue or drain any sends")
			}

			b.ReportMetric(float64(b.N), "broadcasts")
			b.ReportMetric(float64(expectedSends), "attempted_sends")
			b.ReportMetric(float64(enqueued), "enqueued_sends")
			b.ReportMetric(float64(dropped), "dropped_sends")
			b.ReportMetric(float64(drained), "drained_sends")
			b.ReportMetric(float64(enqueued)/b.Elapsed().Seconds(), "enqueued_sends/sec")
			b.ReportMetric(float64(drained)/b.Elapsed().Seconds(), "drained_sends/sec")
		})
	}
}

func BenchmarkStressBroadcastWhileFanInFanOutIncreasingConnections(b *testing.B) {
	defer discardBenchmarkLogs()()

	body, err := json.Marshal(IncomingMessage{Command: "echo", Data: JsonObject{"ok": true}})
	if err != nil {
		b.Fatalf("marshal benchmark message: %v", err)
	}

	for _, connectionCount := range benchmarkConnectionCounts() {
		b.Run(fmt.Sprintf("connections=%d", connectionCount), func(b *testing.B) {
			base := makeStressServer()
			var handled atomic.Int64
			base.AddCommandHandler("echo", func(c connection.WebsocketConnection, data JsonAny) {
				handled.Add(1)
				_ = c.Send(c.Context(), []byte("fanout-echo"))
			})

			messagesPerConnection := max(1, (b.N+connectionCount-1)/connectionCount)
			totalMessages := messagesPerConnection * connectionCount
			conns := makeBenchmarkConnections(base, connectionCount, messagesPerConnection+1)

			var readers sync.WaitGroup
			readers.Add(connectionCount)
			for _, conn := range conns {
				conn := conn
				go func() {
					defer readers.Done()
					base.IncomingCommand(conn)
				}()
			}

			var broadcasts atomic.Int64
			stopBroadcasts := make(chan struct{})

			b.ReportAllocs()
			b.ResetTimer()

			var broadcastWG sync.WaitGroup
			broadcastWG.Add(1)
			go func() {
				defer broadcastWG.Done()
				for {
					select {
					case <-stopBroadcasts:
						return
					default:
						base.Broadcast([]byte("broadcast"), nil)
						broadcasts.Add(1)
					}
				}
			}()

			runConcurrent(connectionCount, func(i int) {
				for j := 0; j < messagesPerConnection; j++ {
					conns[i].incoming <- body
				}
			})
			waitForBenchmarkCount(b, &handled, int64(totalMessages))
			close(stopBroadcasts)
			broadcastWG.Wait()
			waitForBenchmarkWrites(b, conns)
			b.StopTimer()

			closeBenchmarkConnections(base, conns)
			readers.Wait()

			enqueued, dropped, drained := benchmarkConnectionWriteStats(conns)
			if enqueued+drained == 0 {
				b.Fatalf("mixed benchmark did not enqueue or drain any sends")
			}

			b.ReportMetric(float64(totalMessages), "messages")
			b.ReportMetric(float64(broadcasts.Load()), "broadcasts")
			b.ReportMetric(float64(enqueued), "enqueued_sends")
			b.ReportMetric(float64(dropped), "dropped_sends")
			b.ReportMetric(float64(drained), "drained_sends")
			b.ReportMetric(float64(totalMessages)/b.Elapsed().Seconds(), "messages/sec")
			b.ReportMetric(float64(enqueued)/b.Elapsed().Seconds(), "enqueued_sends/sec")
			b.ReportMetric(float64(drained)/b.Elapsed().Seconds(), "drained_sends/sec")
		})
	}
}

func growthBenchmarkDuration() time.Duration {
	if testing.Short() {
		return 3 * time.Second
	}
	return 30 * time.Second
}

func growthBenchmarkResetCycles() int {
	if testing.Short() {
		return 1
	}
	return 3
}

type growthBenchmarkStats struct {
	generations         int64
	totalMessages       int64
	totalBroadcasts     int64
	totalEnqueued       int64
	totalDropped        int64
	totalSends          int64
	maxUsers            int
	maxConnsPerUser     int
	maxConnectionsSeen  int
	slowestGeneration   time.Duration
	minMessagesPerSec   float64
	minSendsPerSec      float64
	resetCycles         int64
	resetGenerations    int64
	resetElapsed        time.Duration
	steadyMaxGeneration int64
}

func (s *growthBenchmarkStats) record(userCount int, connsPerUser int, connectionCount int, broadcasts int64, enqueued int64, dropped int64, sends int64, elapsed time.Duration) {
	messagesPerSecond := float64(connectionCount) / elapsed.Seconds()
	sendsPerSecond := float64(sends) / elapsed.Seconds()
	if s.minMessagesPerSec == 0 || messagesPerSecond < s.minMessagesPerSec {
		s.minMessagesPerSec = messagesPerSecond
	}
	if s.minSendsPerSec == 0 || sendsPerSecond < s.minSendsPerSec {
		s.minSendsPerSec = sendsPerSecond
	}
	if elapsed > s.slowestGeneration {
		s.slowestGeneration = elapsed
	}
	if userCount > s.maxUsers {
		s.maxUsers = userCount
	}
	if connsPerUser > s.maxConnsPerUser {
		s.maxConnsPerUser = connsPerUser
	}
	if connectionCount > s.maxConnectionsSeen {
		s.maxConnectionsSeen = connectionCount
	}

	s.generations++
	s.totalMessages += int64(connectionCount)
	s.totalBroadcasts += broadcasts
	s.totalEnqueued += enqueued
	s.totalDropped += dropped
	s.totalSends += sends
}

func runGrowthBenchmarkGeneration(b *testing.B, body []byte, userCount int, connsPerUser int) (connectionCount int, broadcasts int64, enqueued int64, dropped int64, sends int64, elapsed time.Duration) {
	b.Helper()

	started := time.Now()
	base := makeStressServer()
	var handled atomic.Int64
	base.AddCommandHandler("echo", func(c connection.WebsocketConnection, data JsonAny) {
		handled.Add(1)
		_ = c.Send(c.Context(), []byte("echo"))
	})

	conns := makeBenchmarkUserConnections(base, userCount, connsPerUser, 2)

	var readers sync.WaitGroup
	readers.Add(len(conns))
	for _, conn := range conns {
		conn := conn
		go func() {
			defer readers.Done()
			base.IncomingCommand(conn)
		}()
	}

	var broadcastCount atomic.Int64
	stopBroadcasts := make(chan struct{})
	var broadcastWG sync.WaitGroup
	broadcastWG.Add(1)
	go func() {
		defer broadcastWG.Done()
		for {
			select {
			case <-stopBroadcasts:
				return
			default:
				base.Broadcast([]byte("broadcast"), nil)
				broadcastCount.Add(1)
			}
		}
	}()

	runConcurrent(len(conns), func(i int) {
		conns[i].incoming <- body
	})
	waitForBenchmarkCount(b, &handled, int64(len(conns)))
	close(stopBroadcasts)
	broadcastWG.Wait()
	waitForBenchmarkWrites(b, conns)

	closeBenchmarkConnections(base, conns)
	readers.Wait()

	enqueued, dropped, sends = benchmarkConnectionWriteStats(conns)
	if enqueued+sends == 0 {
		b.Fatalf("growth generation did not enqueue or drain any sends")
	}

	return len(conns), broadcastCount.Load(), enqueued, dropped, sends, time.Since(started)
}

func BenchmarkStressGrowingUsersAndConnectionsFanInFanOut(b *testing.B) {
	defer discardBenchmarkLogs()()

	body, err := json.Marshal(IncomingMessage{Command: "echo", Data: JsonObject{"ok": true}})
	if err != nil {
		b.Fatalf("marshal benchmark message: %v", err)
	}

	const initialUsers = 100
	const initialConnsPerUser = 1
	const maxConnections = 75000

	duration := growthBenchmarkDuration()
	deadline := time.Now().Add(duration)
	userCount := initialUsers
	connsPerUser := initialConnsPerUser

	stats := growthBenchmarkStats{}

	b.ReportAllocs()
	b.ResetTimer()
	runStarted := time.Now()

	for time.Now().Before(deadline) {
		totalConnections := userCount * connsPerUser
		if totalConnections > maxConnections {
			panic("growth benchmark advanced past max connections")
		}

		connectionCount, broadcasts, enqueued, dropped, sends, elapsed := runGrowthBenchmarkGeneration(b, body, userCount, connsPerUser)
		stats.record(userCount, connsPerUser, connectionCount, broadcasts, enqueued, dropped, sends, elapsed)

		nextUsers := userCount * 3 / 2
		nextConnsPerUser := connsPerUser * 2
		if nextUsers*nextConnsPerUser <= maxConnections {
			userCount = nextUsers
			connsPerUser = nextConnsPerUser
		} else {
			stats.steadyMaxGeneration++
		}
	}

	elapsed := time.Since(runStarted)
	b.StopTimer()

	resetStarted := time.Now()
	resetCycles := growthBenchmarkResetCycles()
	for cycle := 0; cycle < resetCycles; cycle++ {
		resetUsers := initialUsers
		resetConnsPerUser := initialConnsPerUser
		for {
			if resetUsers*resetConnsPerUser > maxConnections {
				break
			}
			_, _, _, _, _, _ = runGrowthBenchmarkGeneration(b, body, resetUsers, resetConnsPerUser)
			stats.resetGenerations++

			nextUsers := resetUsers * 3 / 2
			nextConnsPerUser := resetConnsPerUser * 2
			if nextUsers*nextConnsPerUser > maxConnections {
				break
			}
			resetUsers = nextUsers
			resetConnsPerUser = nextConnsPerUser
		}
		stats.resetCycles++
	}
	stats.resetElapsed = time.Since(resetStarted)

	b.ReportMetric(float64(duration/time.Second), "target_seconds")
	b.ReportMetric(float64(stats.generations), "generations")
	b.ReportMetric(float64(stats.steadyMaxGeneration), "steady_max_generations")
	b.ReportMetric(float64(stats.maxUsers), "max_users")
	b.ReportMetric(float64(stats.maxConnsPerUser), "max_conns_per_user")
	b.ReportMetric(float64(stats.maxConnectionsSeen), "max_connections")
	b.ReportMetric(float64(stats.totalMessages), "messages")
	b.ReportMetric(float64(stats.totalBroadcasts), "broadcasts")
	b.ReportMetric(float64(stats.totalEnqueued), "enqueued_sends")
	b.ReportMetric(float64(stats.totalDropped), "dropped_sends")
	b.ReportMetric(float64(stats.totalSends), "drained_sends")
	b.ReportMetric(float64(stats.totalMessages)/elapsed.Seconds(), "messages/sec")
	b.ReportMetric(float64(stats.totalEnqueued)/elapsed.Seconds(), "enqueued_sends/sec")
	b.ReportMetric(float64(stats.totalSends)/elapsed.Seconds(), "drained_sends/sec")
	b.ReportMetric(stats.minMessagesPerSec, "slowest_messages/sec")
	b.ReportMetric(stats.minSendsPerSec, "slowest_drained_sends/sec")
	b.ReportMetric(float64(stats.slowestGeneration.Milliseconds()), "slowest_generation_ms")
	b.ReportMetric(float64(stats.resetCycles), "reset_cycles")
	b.ReportMetric(float64(stats.resetGenerations), "reset_generations")
	b.ReportMetric(float64(stats.resetElapsed.Milliseconds()), "reset_elapsed_ms")
}
