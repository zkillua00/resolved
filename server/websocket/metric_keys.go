package server

const (
	TotalConnections         = "total_connections"
	WaitingConnections       = "waiting_connections"
	GuestConnections         = "guest_connections"
	AuthenticatedConnections = "authenticated_connections"

	TotalMessagesReceived             = "total_messages_received"
	TotalErronousMessagesReceived     = "total_erronous_messages_received"
	TotalConnectionClosedMsgsReceived = "total_connection_closed_messages_received"
	TotalJsonErrorMessagesReceived    = "total_json_error_messages_received"
	TotalReadErrorMessagesReceived    = "total_read_error_messages_received"
	TotalHandledCommands              = "total_handled_commands"
	TotalUnknownCommands              = "total_unknown_commands"

	TotalFailedToBroadcastJsonMessages = "total_failed_to_broadcast_json_messages"
	TotalBroadcastedJsonMessages       = "total_broadcasted_json_messages"
	TotalBroadcastedMsgsToAllChannels  = "total_broadcasted_messages_to_all_channels"
	TotalBroadcastedMsgsToChannel      = "total_broadcasted_messages_to_channel"

	AuthRequestedPrefix = "auth_requested:"
	IpAddressPrefix     = "ip:"
)

var metricKeys = []string{
	TotalConnections,
	WaitingConnections,
	GuestConnections,
	AuthenticatedConnections,
	TotalMessagesReceived,
	TotalErronousMessagesReceived,
	TotalConnectionClosedMsgsReceived,
	TotalJsonErrorMessagesReceived,
	TotalReadErrorMessagesReceived,
	TotalHandledCommands,
	TotalUnknownCommands,
	TotalFailedToBroadcastJsonMessages,
	TotalBroadcastedJsonMessages,
	TotalBroadcastedMsgsToAllChannels,
	TotalBroadcastedMsgsToChannel,
}
