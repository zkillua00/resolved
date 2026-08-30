package realtime

import (
	"encoding/json"
	"testing"
	"time"

	"resolved-server/eventsystem"
	"resolved-server/internal/identity"
	"resolved-server/internal/resourceevents"
	websocket "resolved-server/websocket"
	"resolved-server/websocket/connection"
)

func TestPublisherRequiresResourceScopeAndReadPermissions(t *testing.T) {
	events := eventsystem.NewEventListener()
	events.StartNewEventLoop()
	t.Cleanup(events.Stop)

	socket := websocket.MakeMockWebsocketServer()
	newPublisher(events, socket)

	member := connection.NewMockWebsocketConnection()
	owner := connection.NewMockWebsocketConnection()
	scopedWithoutRead := connection.NewMockWebsocketConnection()
	outsider := connection.NewMockWebsocketConnection()
	socket.AddUser("member", userChannel("member"), member)
	for _, permission := range requiredPermissions(resourceevents.ResourceCollection) {
		socket.AddUser("member", permissionChannel(permission), member)
		socket.AddUser("owner", permissionChannel(permission), owner)
		socket.AddUser("outsider", permissionChannel(permission), outsider)
	}
	socket.AddUser("owner", ownersChannel, owner)
	socket.AddUser("scoped-without-read", userChannel("scoped-without-read"), scopedWithoutRead)
	socket.AddUser("outsider", userChannel("outsider"), outsider)

	resourceevents.Emit(events, resourceevents.Change{
		Resource:     resourceevents.ResourceCollection,
		Action:       resourceevents.ActionUpdated,
		ResourceID:   "collection-1",
		WorkspaceID:  "workspace-1",
		CollectionID: "collection-1",
		Audience: resourceevents.Audience{
			Owners:  true,
			UserIDs: []string{"member", "scoped-without-read"},
		},
	})

	waitForMessages(t, member, 1)
	waitForMessages(t, owner, 1)
	assertMessageCount(t, scopedWithoutRead, 0)
	assertMessageCount(t, outsider, 0)
	assertMessageCount(t, member, 1)

	member.Mu.Lock()
	body := append([]byte(nil), member.ReceivedMessages[0]...)
	member.Mu.Unlock()
	var received struct {
		Command string                `json:"command"`
		Data    resourceevents.Change `json:"data"`
	}
	if err := json.Unmarshal(body, &received); err != nil {
		t.Fatalf("decode published event: %v", err)
	}
	if received.Command != commandResourceChanged {
		t.Fatalf("command = %q, want %q", received.Command, commandResourceChanged)
	}
	if received.Data.EventID == "" || received.Data.OccurredAt.IsZero() {
		t.Fatalf("event metadata was not populated: %+v", received.Data)
	}
	if received.Data.ResourceID != "collection-1" || received.Data.WorkspaceID != "workspace-1" {
		t.Fatalf("unexpected event scope: %+v", received.Data)
	}
	if received.Data.Audience.Everyone || received.Data.Audience.Owners ||
		len(received.Data.Audience.UserIDs) != 0 || len(received.Data.Audience.RoleIDs) != 0 ||
		len(received.Data.Audience.PermissionKeys) != 0 {
		t.Fatalf("private audience leaked into the published payload: %+v", received.Data.Audience)
	}
}

func TestRequiredPermissionsFollowReadEndpoints(t *testing.T) {
	tests := []struct {
		resource resourceevents.Resource
		want     []string
	}{
		{
			resource: resourceevents.ResourceCollection,
			want: []string{
				identity.PermissionWorkspacesRead,
				identity.PermissionCollectionsRead,
				identity.PermissionRequestsRead,
			},
		},
		{resource: resourceevents.ResourceRequest, want: []string{identity.PermissionRequestsRead}},
		{resource: resourceevents.ResourceEnvironment, want: []string{identity.PermissionEnvironmentsRead}},
		{resource: resourceevents.ResourceUser, want: []string{identity.PermissionUsersRead}},
		{resource: resourceevents.ResourceRole, want: []string{identity.PermissionRolesRead}},
		{resource: resourceevents.ResourceRequestExecution, want: []string{identity.PermissionAuditRead}},
		{resource: resourceevents.ResourceServerSettings, want: []string{identity.PermissionServerSettingsRead}},
		{resource: resourceevents.ResourceSharedHistory, want: nil},
	}

	for _, test := range tests {
		t.Run(string(test.resource), func(t *testing.T) {
			got := requiredPermissions(test.resource)
			if len(got) != len(test.want) {
				t.Fatalf("permissions = %v, want %v", got, test.want)
			}
			for index := range test.want {
				if got[index] != test.want[index] {
					t.Fatalf("permissions = %v, want %v", got, test.want)
				}
			}
		})
	}
}

func TestAudienceChannelsAreStableAndDeduplicated(t *testing.T) {
	got := audienceChannels(resourceevents.Audience{
		Everyone:       true,
		Owners:         true,
		UserIDs:        []string{"user-2", "", "user-1", "user-2"},
		RoleIDs:        []string{"role-2", "", "role-2"},
		PermissionKeys: []string{"roles.read", "roles.read", ""},
	})
	want := []string{
		authenticatedChannel,
		ownersChannel,
		permissionChannel("roles.read"),
		roleChannel("role-2"),
		userChannel("user-1"),
		userChannel("user-2"),
	}
	if len(got) != len(want) {
		t.Fatalf("channels = %v, want %v", got, want)
	}
	for index := range want {
		if got[index] != want[index] {
			t.Fatalf("channels = %v, want %v", got, want)
		}
	}
}

func TestIdentityChangesInvalidateAffectedConnections(t *testing.T) {
	socket := websocket.MakeMockWebsocketServer()
	publisher := newPublisher(nil, socket)
	first := connection.NewMockWebsocketConnection()
	second := connection.NewMockWebsocketConnection()
	socket.AddUser("first", authenticatedChannel, first)
	socket.AddUser("first", userChannel("first"), first)
	socket.AddUser("second", authenticatedChannel, second)
	socket.AddUser("second", userChannel("second"), second)

	publisher.publish(resourceevents.Change{
		Resource:   resourceevents.ResourceUser,
		Action:     resourceevents.ActionUpdated,
		ResourceID: "first",
		Audience:   resourceevents.Audience{UserIDs: []string{"first"}},
	})
	if !first.Closed() {
		t.Fatal("the changed user's connection remained active")
	}
	if second.Closed() {
		t.Fatal("an unrelated user's connection was closed")
	}

	assigned := connection.NewMockWebsocketConnection()
	unassigned := connection.NewMockWebsocketConnection()
	socket.AddUser("assigned", roleChannel("role-1"), assigned)
	socket.AddUser("unassigned", roleChannel("role-2"), unassigned)
	publisher.publish(resourceevents.Change{
		Resource:   resourceevents.ResourceRole,
		Action:     resourceevents.ActionUpdated,
		ResourceID: "role-1",
		Audience:   resourceevents.Audience{RoleIDs: []string{"role-1"}},
	})
	if !assigned.Closed() {
		t.Fatal("a role change did not invalidate a user assigned to that role")
	}
	if unassigned.Closed() {
		t.Fatal("a role change invalidated a user who is not assigned to that role")
	}
}

func waitForMessages(t *testing.T, client *connection.MockWebsocketConnection, count int) {
	t.Helper()
	deadline := time.Now().Add(time.Second)
	for time.Now().Before(deadline) {
		client.Mu.Lock()
		actual := len(client.ReceivedMessages)
		client.Mu.Unlock()
		if actual >= count {
			return
		}
		time.Sleep(time.Millisecond)
	}
	assertMessageCount(t, client, count)
}

func assertMessageCount(t *testing.T, client *connection.MockWebsocketConnection, want int) {
	t.Helper()
	client.Mu.Lock()
	defer client.Mu.Unlock()
	if got := len(client.ReceivedMessages); got != want {
		t.Fatalf("message count = %d, want %d", got, want)
	}
}
