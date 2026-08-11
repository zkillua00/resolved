package realtime

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"sort"
	"time"

	"github.com/gofiber/fiber/v3"

	"resolved-server/eventsystem"
	"resolved-server/internal/auth"
	"resolved-server/internal/identity"
	"resolved-server/internal/resourceevents"
	websocket "resolved-server/websocket"
	"resolved-server/websocket/connection"
)

const (
	commandResourceChanged = "resource.changed"
	commandPing            = "ping"
	commandPong            = "pong"

	authenticatedChannel  = "authenticated"
	ownersChannel         = "owners"
	connectionUserKey     = "realtime_user_id"
	connectionChannelsKey = "realtime_channels"
	connectionExpiryKey   = "realtime_expiry_timer"
)

type principalContextKey struct{}

type message struct {
	Command string                `json:"command"`
	Data    resourceevents.Change `json:"data"`
}

type Publisher struct {
	socket websocket.WebsocketServer
}

func New(events *eventsystem.EventListener) *Publisher {
	return newPublisher(events, websocket.MakeGorillaWebsocketServer())
}

func newPublisher(events *eventsystem.EventListener, socket websocket.WebsocketServer) *Publisher {
	publisher := &Publisher{socket: socket}
	publisher.configureConnections()
	if events != nil {
		events.RegisterEventHandler(
			resourceevents.EventName,
			eventsystem.UseEventAs(func(change resourceevents.Change) error {
				publisher.publish(change)
				return nil
			}, true),
		)
	}
	return publisher
}

func (p *Publisher) Handler() fiber.Handler {
	handler := p.socket.FiberHandler()
	return func(c fiber.Ctx) error {
		principal := auth.PrincipalFromContext(c)
		if principal == nil {
			return fmt.Errorf("authenticated websocket route is missing a principal")
		}
		c.SetContext(context.WithValue(c.Context(), principalContextKey{}, principal))
		return handler(c)
	}
}

func (p *Publisher) configureConnections() {
	p.socket.AddOnActiveHandler(func(client connection.WebsocketConnection) {
		principal, _ := client.Context().Value(principalContextKey{}).(*auth.Principal)
		if principal == nil {
			p.socket.CloseConnection(client)
			return
		}
		expiresIn := time.Until(principal.ExpiresAt())
		if expiresIn <= 0 {
			p.socket.CloseConnection(client)
			return
		}

		channels := principalChannels(principal)
		client.Local(connectionUserKey, principal.User.ID)
		client.Local(connectionChannelsKey, channels)
		for _, channel := range channels {
			p.socket.AddUser(principal.User.ID, channel, client)
		}
		expiryTimer := time.AfterFunc(expiresIn, func() {
			p.socket.CloseConnection(client)
		})
		client.Local(connectionExpiryKey, expiryTimer)
	})
	p.socket.AddOnInactiveHandler(func(client connection.WebsocketConnection) {
		if timer, ok := client.Local(connectionExpiryKey, nil); ok {
			if expiryTimer, ok := timer.(*time.Timer); ok {
				expiryTimer.Stop()
			}
		}
		userID, userOK := client.Local(connectionUserKey, nil)
		channels, channelsOK := client.Local(connectionChannelsKey, nil)
		if !userOK || !channelsOK {
			return
		}
		userIDString, userOK := userID.(string)
		channelList, channelsOK := channels.([]string)
		if !userOK || !channelsOK {
			return
		}
		for _, channel := range channelList {
			p.socket.RemoveUser(client, userIDString, channel)
		}
	})
	p.socket.AddCommandHandler(commandPing, func(client connection.WebsocketConnection, _ websocket.JsonAny) {
		_ = client.SendJson(client.Context(), map[string]any{"command": commandPong})
	})
}

func principalChannels(principal *auth.Principal) []string {
	channels := []string{authenticatedChannel, userChannel(principal.User.ID)}
	if principal.HasRole(identity.OwnerRoleID) {
		channels = append(channels, ownersChannel)
	}
	for _, permission := range principal.PermissionKeys() {
		channels = append(channels, permissionChannel(permission))
	}
	for _, role := range principal.User.Roles {
		channels = append(channels, roleChannel(role.ID))
	}
	sort.Strings(channels)
	return channels
}

func (p *Publisher) publish(change resourceevents.Change) {
	body, err := json.Marshal(message{Command: commandResourceChanged, Data: change})
	if err != nil {
		log.Printf("marshal realtime resource event: %v", err)
		return
	}

	seen := make(map[string]struct{})
	for _, channel := range audienceChannels(change.Audience) {
		hub, ok := p.socket.GetChannel(channel)
		if !ok {
			continue
		}
		for _, client := range hub.Snapshot() {
			if _, exists := seen[client.Id()]; exists {
				continue
			}
			seen[client.Id()] = struct{}{}
			if err := client.Send(client.Context(), body); err != nil {
				log.Printf("publish realtime resource event to %s: %v", client.Id(), err)
			}
		}
	}
	p.invalidateAuthorization(change)
}

func (p *Publisher) invalidateAuthorization(change resourceevents.Change) {
	var channel string
	switch change.Resource {
	case resourceevents.ResourceRole:
		channel = roleChannel(change.ResourceID)
	case resourceevents.ResourceUser:
		channel = userChannel(change.ResourceID)
	default:
		return
	}
	hub, ok := p.socket.GetChannel(channel)
	if !ok {
		return
	}
	for _, client := range hub.Snapshot() {
		p.socket.CloseConnection(client)
	}
}

func audienceChannels(audience resourceevents.Audience) []string {
	channels := make(map[string]struct{})
	if audience.Everyone {
		channels[authenticatedChannel] = struct{}{}
	}
	if audience.Owners {
		channels[ownersChannel] = struct{}{}
	}
	for _, userID := range audience.UserIDs {
		if userID != "" {
			channels[userChannel(userID)] = struct{}{}
		}
	}
	for _, roleID := range audience.RoleIDs {
		if roleID != "" {
			channels[roleChannel(roleID)] = struct{}{}
		}
	}
	for _, permission := range audience.PermissionKeys {
		if permission != "" {
			channels[permissionChannel(permission)] = struct{}{}
		}
	}
	result := make([]string, 0, len(channels))
	for channel := range channels {
		result = append(result, channel)
	}
	sort.Strings(result)
	return result
}

func userChannel(userID string) string {
	return "user:" + userID
}

func permissionChannel(permission string) string {
	return "permission:" + permission
}

func roleChannel(roleID string) string {
	return "role:" + roleID
}
