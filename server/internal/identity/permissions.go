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
}

func PermissionCatalog() []Permission {
	permissions := make([]Permission, len(permissionCatalog))
	copy(permissions, permissionCatalog)
	return permissions
}
