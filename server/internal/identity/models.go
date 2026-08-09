package identity

import "time"

const OwnerRoleID = "00000000-0000-0000-0000-000000000001"

type User struct {
	ID           string    `gorm:"type:char(36);primaryKey"`
	Email        string    `gorm:"size:254;uniqueIndex;not null"`
	DisplayName  string    `gorm:"size:120;not null"`
	PasswordHash string    `gorm:"size:255;not null"`
	Active       bool      `gorm:"not null;default:true"`
	Roles        []Role    `gorm:"many2many:user_roles;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	CreatedAt    time.Time `gorm:"not null"`
	UpdatedAt    time.Time `gorm:"not null"`
}

type Role struct {
	ID             string       `gorm:"type:char(36);primaryKey"`
	Name           string       `gorm:"size:100;not null"`
	NormalizedName string       `gorm:"size:100;uniqueIndex;not null"`
	Description    string       `gorm:"size:500;not null;default:''"`
	System         bool         `gorm:"not null;default:false"`
	Permissions    []Permission `gorm:"many2many:role_permissions;constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	CreatedAt      time.Time    `gorm:"not null"`
	UpdatedAt      time.Time    `gorm:"not null"`
}

type Permission struct {
	Key         string `gorm:"size:100;primaryKey"`
	Description string `gorm:"size:255;not null"`
}

type Session struct {
	ID        string    `gorm:"type:char(36);primaryKey"`
	TokenHash string    `gorm:"size:64;uniqueIndex;not null"`
	UserID    string    `gorm:"type:char(36);not null;index"`
	User      User      `gorm:"constraint:OnUpdate:CASCADE,OnDelete:CASCADE"`
	ExpiresAt time.Time `gorm:"not null;index"`
	CreatedAt time.Time `gorm:"not null"`
}

type BootstrapState struct {
	Key       string    `gorm:"size:50;primaryKey"`
	CreatedAt time.Time `gorm:"not null"`
}
