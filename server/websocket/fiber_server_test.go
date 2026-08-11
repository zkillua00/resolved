package server

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"testing"
	"time"

	"github.com/gofiber/fiber/v3"
	gorillaWebsocket "github.com/gorilla/websocket"

	"resolved-server/websocket/connection"
)

type fiberContextKey struct{}

func TestGorillaWebsocketServerFiberHandler(t *testing.T) {
	ws := MakeGorillaWebsocketServer()
	ws.AddCommandHandler("inspect", func(c connection.WebsocketConnection, _ JsonAny) {
		header, _ := c.Header("X-Test-Header")
		contextValue, _ := c.Context().Value(fiberContextKey{}).(string)
		if err := c.SendJson(c.Context(), JsonObject{
			"header":  header,
			"context": contextValue,
		}); err != nil {
			t.Errorf("send response: %v", err)
		}
	})

	app := fiber.New()
	app.Use(func(c fiber.Ctx) error {
		c.SetContext(context.WithValue(c.Context(), fiberContextKey{}, "from-fiber"))
		return c.Next()
	})
	app.Get("/socket", ws.FiberHandler())

	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}

	serverErr := make(chan error, 1)
	go func() {
		serverErr <- app.Listener(listener, fiber.ListenConfig{DisableStartupMessage: true})
	}()

	var client *gorillaWebsocket.Conn
	t.Cleanup(func() {
		if client != nil {
			_ = client.Close()
		}
		if err := app.ShutdownWithTimeout(2 * time.Second); err != nil {
			t.Errorf("shutdown Fiber app: %v", err)
		}
		select {
		case err := <-serverErr:
			if err != nil {
				t.Errorf("serve Fiber app: %v", err)
			}
		case <-time.After(2 * time.Second):
			t.Error("Fiber app did not stop")
		}
	})

	headers := http.Header{"X-Test-Header": []string{"through-fiber"}}
	client, response, err := gorillaWebsocket.DefaultDialer.Dial(
		fmt.Sprintf("ws://%s/socket", listener.Addr().String()),
		headers,
	)
	if err != nil {
		if response != nil {
			t.Fatalf("dial websocket: %v (status %s)", err, response.Status)
		}
		t.Fatalf("dial websocket: %v", err)
	}

	if err := client.SetReadDeadline(time.Now().Add(2 * time.Second)); err != nil {
		t.Fatalf("set read deadline: %v", err)
	}
	if err := client.WriteJSON(IncomingMessage{Command: "inspect"}); err != nil {
		t.Fatalf("write command: %v", err)
	}

	var result JsonObject
	if err := client.ReadJSON(&result); err != nil {
		t.Fatalf("read command response: %v", err)
	}
	if got := result["header"]; got != "through-fiber" {
		t.Errorf("header = %v, want through-fiber", got)
	}
	if got := result["context"]; got != "from-fiber" {
		t.Errorf("context = %v, want from-fiber", got)
	}
}
