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
}

func PermissionCatalog() []Permission {
	permissions := make([]Permission, len(permissionCatalog))
	copy(permissions, permissionCatalog)
	return permissions
}
