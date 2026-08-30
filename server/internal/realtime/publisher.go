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

	invalidated := p.invalidateAuthorization(change)
	permitted := p.permittedConnections(change.Resource)
	seen := make(map[string]struct{})
	for _, channel := range audienceChannels(change.Audience) {
		hub, ok := p.socket.GetChannel(channel)
		if !ok {
			continue
		}
		for _, client := range hub.Snapshot() {
			if _, blocked := invalidated[client.Id()]; blocked {
				continue
			}
			if permitted != nil {
				if _, allowed := permitted[client.Id()]; !allowed {
					continue
				}
			}
			if _, exists := seen[client.Id()]; exists {
				continue
			}
			seen[client.Id()] = struct{}{}
			if err := client.Send(client.Context(), body); err != nil {
				log.Printf("publish realtime resource event to %s: %v", client.Id(), err)
			}
		}
	}
}

func (p *Publisher) invalidateAuthorization(change resourceevents.Change) map[string]struct{} {
	var channel string
	switch change.Resource {
	case resourceevents.ResourceRole:
		channel = roleChannel(change.ResourceID)
	case resourceevents.ResourceUser:
		channel = userChannel(change.ResourceID)
	default:
		return nil
	}
	hub, ok := p.socket.GetChannel(channel)
	if !ok {
		return nil
	}
	invalidated := make(map[string]struct{})
	for _, client := range hub.Snapshot() {
		invalidated[client.Id()] = struct{}{}
		p.socket.CloseConnection(client)
	}
	return invalidated
}

// permittedConnections intersects every resource event with the permissions
// required to load that resource through the HTTP API. Audience channels
// select the account/workspace/collection scope; permission channels are a
// second, mandatory gate rather than another way to enter the audience.
func (p *Publisher) permittedConnections(resource resourceevents.Resource) map[string]struct{} {
	permissions := requiredPermissions(resource)
	if len(permissions) == 0 {
		return nil
	}

	var permitted map[string]struct{}
	for _, permission := range permissions {
		hub, ok := p.socket.GetChannel(permissionChannel(permission))
		if !ok {
			return map[string]struct{}{}
		}
		current := make(map[string]struct{})
		for _, client := range hub.Snapshot() {
			current[client.Id()] = struct{}{}
		}
		if permitted == nil {
			permitted = current
			continue
		}
		for connectionID := range permitted {
			if _, ok := current[connectionID]; !ok {
				delete(permitted, connectionID)
			}
		}
	}
	return permitted
}

func requiredPermissions(resource resourceevents.Resource) []string {
	switch resource {
	case resourceevents.ResourceUser:
		return []string{identity.PermissionUsersRead}
	case resourceevents.ResourceRole:
		return []string{identity.PermissionRolesRead}
	case resourceevents.ResourceWorkspace, resourceevents.ResourceCollection:
		// There is no unscoped collection-list endpoint. The desktop client
		// lists the permission-projected tree through GET /workspaces.
		return []string{
			identity.PermissionWorkspacesRead,
			identity.PermissionCollectionsRead,
			identity.PermissionRequestsRead,
		}
	case resourceevents.ResourceRequest:
		return []string{identity.PermissionRequestsRead}
	case resourceevents.ResourceEnvironment, resourceevents.ResourceEnvironmentVariable:
		return []string{identity.PermissionEnvironmentsRead}
	case resourceevents.ResourceRequestExecution:
		return []string{identity.PermissionAuditRead}
	case resourceevents.ResourceServerSettings:
		return []string{identity.PermissionServerSettingsRead}
	default:
		// Shared-history recipients are resolved by a database query that
		// already combines workspace scope with history.read_others, while
		// still allowing the history owner to receive their own updates.
		return nil
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
