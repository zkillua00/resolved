package workspaces

import (
	"time"

	"resolved-server/internal/identity"
)

// Workspace is the top-level collaboration boundary. UserIDs and Collections
// are hydrated aggregates and are not stored as columns on the workspace row.
type Workspace struct {
	ID          string       `gorm:"type:char(36);primaryKey"`
	Name        string       `gorm:"size:120;not null"`
	UserIDs     []string     `gorm:"-"`
	Collections []Collection `gorm:"-"`
	CreatedAt   time.Time    `gorm:"not null"`
	UpdatedAt   time.Time    `gorm:"not null"`
}

// Collection is one node in a workspace collection tree. A nil
// ParentCollectionID places the node at the workspace root.
type Collection struct {
	ID                 string       `gorm:"type:char(36);primaryKey"`
	WorkspaceID        string       `gorm:"type:char(36);not null;index"`
	Workspace          Workspace    `gorm:"foreignKey:WorkspaceID;references:ID;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	ParentCollectionID *string      `gorm:"type:char(36);index"`
	Name               string       `gorm:"size:120;not null"`
	UserIDs            []string     `gorm:"-"`
	SubCollections     []Collection `gorm:"-"`
	CreatedAt          time.Time    `gorm:"not null"`
	UpdatedAt          time.Time    `gorm:"not null"`
}

// WorkspaceUser is a direct workspace grant. Collection grants are not
// expanded into this table.
type WorkspaceUser struct {
	WorkspaceID string        `gorm:"type:char(36);primaryKey"`
	Workspace   Workspace     `gorm:"foreignKey:WorkspaceID;references:ID;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	UserID      string        `gorm:"type:char(36);primaryKey;index"`
	User        identity.User `gorm:"foreignKey:UserID;references:ID;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	CreatedAt   time.Time     `gorm:"not null"`
}

// CollectionUser is a direct grant on one collection node. Its effective
// scope includes that collection and every descendant.
type CollectionUser struct {
	CollectionID string        `gorm:"type:char(36);primaryKey"`
	Collection   Collection    `gorm:"foreignKey:CollectionID;references:ID;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	UserID       string        `gorm:"type:char(36);primaryKey;index"`
	User         identity.User `gorm:"foreignKey:UserID;references:ID;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	CreatedAt    time.Time     `gorm:"not null"`
}

type WorkspaceView struct {
	ID          string           `json:"id"`
	Name        string           `json:"name"`
	UserIDs     []string         `json:"user_ids"`
	Collections []CollectionView `json:"collections"`
	CreatedAt   time.Time        `json:"created_at"`
	UpdatedAt   time.Time        `json:"updated_at"`
}

type CollectionView struct {
	ID                 string           `json:"id"`
	WorkspaceID        string           `json:"workspace_id"`
	ParentCollectionID *string          `json:"parent_collection_id"`
	Name               string           `json:"name"`
	UserIDs            []string         `json:"user_ids"`
	SubCollections     []CollectionView `json:"sub_collections"`
	CreatedAt          time.Time        `json:"created_at"`
	UpdatedAt          time.Time        `json:"updated_at"`
}

func ViewWorkspace(workspace Workspace) WorkspaceView {
	return WorkspaceView{
		ID:          workspace.ID,
		Name:        workspace.Name,
		UserIDs:     cloneStrings(workspace.UserIDs),
		Collections: ViewCollections(workspace.Collections),
		CreatedAt:   workspace.CreatedAt,
		UpdatedAt:   workspace.UpdatedAt,
	}
}

func ViewWorkspaces(workspaces []Workspace) []WorkspaceView {
	views := make([]WorkspaceView, 0, len(workspaces))
	for _, workspace := range workspaces {
		views = append(views, ViewWorkspace(workspace))
	}
	return views
}

func ViewCollection(collection Collection) CollectionView {
	return CollectionView{
		ID:                 collection.ID,
		WorkspaceID:        collection.WorkspaceID,
		ParentCollectionID: collection.ParentCollectionID,
		Name:               collection.Name,
		UserIDs:            cloneStrings(collection.UserIDs),
		SubCollections:     ViewCollections(collection.SubCollections),
		CreatedAt:          collection.CreatedAt,
		UpdatedAt:          collection.UpdatedAt,
	}
}

func ViewCollections(collections []Collection) []CollectionView {
	views := make([]CollectionView, 0, len(collections))
	for _, collection := range collections {
		views = append(views, ViewCollection(collection))
	}
	return views
}

func cloneStrings(values []string) []string {
	if len(values) == 0 {
		return []string{}
	}
	return append([]string(nil), values...)
}
