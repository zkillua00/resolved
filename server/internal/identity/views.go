package identity

import (
	"sort"
	"time"
)

type PermissionView struct {
	Key         string `json:"key"`
	Description string `json:"description"`
}

type UserSummaryView struct {
	ID          string `json:"id"`
	Email       string `json:"email"`
	DisplayName string `json:"display_name"`
}

type RoleView struct {
	ID          string           `json:"id"`
	Name        string           `json:"name"`
	Description string           `json:"description"`
	System      bool             `json:"system"`
	Permissions []PermissionView `json:"permissions"`
	CreatedBy   *UserSummaryView `json:"created_by"`
	CreatedAt   time.Time        `json:"created_at"`
	UpdatedAt   time.Time        `json:"updated_at"`
}

type UserView struct {
	ID          string           `json:"id"`
	Email       string           `json:"email"`
	DisplayName string           `json:"display_name"`
	Active      bool             `json:"active"`
	Roles       []RoleView       `json:"roles"`
	CreatedBy   *UserSummaryView `json:"created_by"`
	CreatedAt   time.Time        `json:"created_at"`
	UpdatedAt   time.Time        `json:"updated_at"`
}

func ViewUserSummary(user *User) *UserSummaryView {
	if user == nil {
		return nil
	}
	return &UserSummaryView{
		ID:          user.ID,
		Email:       user.Email,
		DisplayName: user.DisplayName,
	}
}

func ViewPermission(permission Permission) PermissionView {
	return PermissionView{
		Key:         permission.Key,
		Description: permission.Description,
	}
}

func ViewPermissions(permissions []Permission) []PermissionView {
	views := make([]PermissionView, 0, len(permissions))
	for _, permission := range permissions {
		views = append(views, ViewPermission(permission))
	}
	sort.Slice(views, func(i, j int) bool { return views[i].Key < views[j].Key })
	return views
}

func ViewRole(role Role) RoleView {
	return RoleView{
		ID:          role.ID,
		Name:        role.Name,
		Description: role.Description,
		System:      role.System,
		Permissions: ViewPermissions(role.Permissions),
		CreatedBy:   ViewUserSummary(role.CreatedByUser),
		CreatedAt:   role.CreatedAt,
		UpdatedAt:   role.UpdatedAt,
	}
}

func ViewRoles(roles []Role) []RoleView {
	views := make([]RoleView, 0, len(roles))
	for _, role := range roles {
		views = append(views, ViewRole(role))
	}
	sort.Slice(views, func(i, j int) bool {
		return views[i].Name < views[j].Name
	})
	return views
}

func ViewUser(user User) UserView {
	return UserView{
		ID:          user.ID,
		Email:       user.Email,
		DisplayName: user.DisplayName,
		Active:      user.Active,
		Roles:       ViewRoles(user.Roles),
		CreatedBy:   ViewUserSummary(user.CreatedByUser),
		CreatedAt:   user.CreatedAt,
		UpdatedAt:   user.UpdatedAt,
	}
}

func ViewUsers(users []User) []UserView {
	views := make([]UserView, 0, len(users))
	for _, user := range users {
		views = append(views, ViewUser(user))
	}
	return views
}
