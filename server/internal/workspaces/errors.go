package workspaces

import "errors"

var (
	ErrWorkspaceNotFound            = errors.New("workspace not found")
	ErrCollectionNotFound           = errors.New("collection not found")
	ErrSavedRequestNotFound         = errors.New("saved request not found")
	ErrParentCollectionMissing      = errors.New("parent collection not found in workspace")
	ErrUnknownUser                  = errors.New("one or more users do not exist")
	ErrCollectionCycle              = errors.New("collection tree would contain a cycle")
	ErrEnvironmentNotFound          = errors.New("environment not found")
	ErrEnvironmentVariableNotFound  = errors.New("environment variable not found")
	ErrEnvironmentVariableKeyExists = errors.New("environment variable key already exists")
)
