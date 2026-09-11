package identity

const (
	PermissionUsersRead        = "users.read"
	PermissionUsersCreate      = "users.create"
	PermissionUsersUpdate      = "users.update"
	PermissionUsersAssignRoles = "users.roles.assign"

	PermissionRolesRead              = "roles.read"
	PermissionRolesCreate            = "roles.create"
	PermissionRolesUpdate            = "roles.update"
	PermissionRolesAssignPermissions = "roles.permissions.assign"

	PermissionPermissionsRead = "permissions.read"

	PermissionWorkspacesRead        = "workspaces.read"
	PermissionWorkspacesCreate      = "workspaces.create"
	PermissionWorkspacesUpdate      = "workspaces.update"
	PermissionWorkspacesDelete      = "workspaces.delete"
	PermissionWorkspacesAssignUsers = "workspaces.users.assign"

	PermissionCollectionsRead        = "collections.read"
	PermissionCollectionsCreate      = "collections.create"
	PermissionCollectionsUpdate      = "collections.update"
	PermissionCollectionsDelete      = "collections.delete"
	PermissionCollectionsAssignUsers = "collections.users.assign"

	PermissionRequestsRead    = "requests.read"
	PermissionRequestsCreate  = "requests.create"
	PermissionRequestsUpdate  = "requests.update"
	PermissionRequestsDelete  = "requests.delete"
	PermissionRequestsExecute = "requests.execute"

	PermissionHistoryReadOthers = "history.read_others"
	PermissionAuditRead         = "audit.read"

	PermissionServerSettingsRead   = "server_settings.read"
	PermissionServerSettingsUpdate = "server_settings.update"

	PermissionProxiesRead   = "proxies.read"
	PermissionProxiesCreate = "proxies.create"
	PermissionProxiesUpdate = "proxies.update"
	PermissionProxiesDelete = "proxies.delete"
	PermissionProxiesAssign = "proxies.assign"

	PermissionEnvironmentsRead        = "environments.read"
	PermissionEnvironmentsCreate      = "environments.create"
	PermissionEnvironmentsUpdate      = "environments.update"
	PermissionEnvironmentsDelete      = "environments.delete"
	PermissionEnvironmentValuesUpdate = "environment_values.update"
)

var permissionCatalog = []Permission{
	{Key: PermissionUsersRead, Description: "View users and their role assignments"},
	{Key: PermissionUsersCreate, Description: "Create users"},
	{Key: PermissionUsersUpdate, Description: "Update users, passwords, and active state"},
	{Key: PermissionUsersAssignRoles, Description: "Replace a user's role assignments"},
	{Key: PermissionRolesRead, Description: "View roles and their permissions"},
	{Key: PermissionRolesCreate, Description: "Create roles"},
	{Key: PermissionRolesUpdate, Description: "Update non-system roles"},
	{Key: PermissionRolesAssignPermissions, Description: "Replace a non-system role's permissions"},
	{Key: PermissionPermissionsRead, Description: "View the permission catalog"},
	{Key: PermissionWorkspacesRead, Description: "View accessible workspaces and their collection trees"},
	{Key: PermissionWorkspacesCreate, Description: "Create workspaces"},
	{Key: PermissionWorkspacesUpdate, Description: "Update workspaces within the account's access scope"},
	{Key: PermissionWorkspacesDelete, Description: "Delete workspaces within the account's access scope"},
	{Key: PermissionWorkspacesAssignUsers, Description: "Replace direct workspace user grants"},
	{Key: PermissionCollectionsRead, Description: "View collection subtrees within the account's access scope"},
	{Key: PermissionCollectionsCreate, Description: "Create collections within the account's access scope"},
	{Key: PermissionCollectionsUpdate, Description: "Rename and move collections within the account's access scope"},
	{Key: PermissionCollectionsDelete, Description: "Delete collection subtrees within the account's access scope"},
	{Key: PermissionCollectionsAssignUsers, Description: "Replace direct collection user grants"},
	{Key: PermissionRequestsRead, Description: "View saved requests within accessible collections"},
	{Key: PermissionRequestsCreate, Description: "Create saved requests within accessible collections"},
	{Key: PermissionRequestsUpdate, Description: "Update saved requests within accessible collections"},
	{Key: PermissionRequestsDelete, Description: "Delete saved requests within accessible collections"},
	{Key: PermissionRequestsExecute, Description: "Run requests from accessible workspaces through this server"},
	{Key: PermissionHistoryReadOthers, Description: "View other users' shared request history in accessible workspaces"},
	{Key: PermissionAuditRead, Description: "View the user and role audit log"},
	{Key: PermissionServerSettingsRead, Description: "View server request execution settings and the destination allowlist"},
	{Key: PermissionServerSettingsUpdate, Description: "Update server request execution settings and the destination allowlist"},
	{Key: PermissionProxiesRead, Description: "View proxies, their host override rules, assignments, and exclusions"},
	{Key: PermissionProxiesCreate, Description: "Create proxies"},
	{Key: PermissionProxiesUpdate, Description: "Update proxy names and host override rules"},
	{Key: PermissionProxiesDelete, Description: "Delete proxies"},
	{Key: PermissionProxiesAssign, Description: "Replace proxy scope assignments and user or role exclusions"},
	{Key: PermissionEnvironmentsRead, Description: "View workspace environment definitions and the account's own values"},
	{Key: PermissionEnvironmentsCreate, Description: "Create environments within directly accessible workspaces"},
	{Key: PermissionEnvironmentsUpdate, Description: "Update environment names and shared variable definitions"},
	{Key: PermissionEnvironmentsDelete, Description: "Delete environments and shared variable definitions"},
	{Key: PermissionEnvironmentValuesUpdate, Description: "Update the account's own environment variable values"},
}

func PermissionCatalog() []Permission {
	permissions := make([]Permission, len(permissionCatalog))
	copy(permissions, permissionCatalog)
	return permissions
}
