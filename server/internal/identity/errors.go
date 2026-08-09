package identity

import "errors"

var (
	ErrUserNotFound        = errors.New("user not found")
	ErrRoleNotFound        = errors.New("role not found")
	ErrEmailExists         = errors.New("email already exists")
	ErrRoleNameExists      = errors.New("role name already exists")
	ErrUnknownRole         = errors.New("one or more roles do not exist")
	ErrUnknownPermission   = errors.New("one or more permissions do not exist")
	ErrLastOwner           = errors.New("the final active owner cannot be removed")
	ErrUsersExist          = errors.New("the deployment already has users")
	ErrSystemRoleImmutable = errors.New("system roles cannot be changed")
)
