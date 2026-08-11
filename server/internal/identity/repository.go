package identity

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"sort"
	"time"

	"gorm.io/gorm"
)

type Repository struct {
	db *gorm.DB
}

type FirstOwnerSetup func(context.Context, *gorm.DB, User) error

type UserChanges struct {
	Email        *string
	DisplayName  *string
	PasswordHash *string
	Active       *bool
}

// BeforePasswordChange runs inside the same transaction as the password,
// environment-value re-encryption, and session updates.
type BeforePasswordChange func(context.Context, *gorm.DB, User, *UserChanges) error

type RoleChanges struct {
	Name           *string
	NormalizedName *string
	Description    *string
}

func NewRepository(db *gorm.DB) *Repository {
	return &Repository{db: db}
}

func (r *Repository) FindUserByEmail(ctx context.Context, email string) (User, error) {
	var user User
	err := r.db.WithContext(ctx).Where("email = ?", email).First(&user).Error
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return User{}, ErrUserNotFound
	}
	return user, err
}

func (r *Repository) GetUser(ctx context.Context, id string) (User, error) {
	var user User
	db := r.db.WithContext(ctx)
	err := preloadUser(db).First(&user, "id = ?", id).Error
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return User{}, ErrUserNotFound
	}
	if err == nil {
		err = hydrateUserCreators(db, &user)
	}
	return user, err
}

func (r *Repository) ListUsers(ctx context.Context) ([]User, error) {
	var users []User
	db := r.db.WithContext(ctx)
	err := preloadUser(db).Order("email ASC").Find(&users).Error
	if err == nil {
		err = hydrateUsersCreators(db, users)
	}
	return users, err
}

func (r *Repository) CreateUser(ctx context.Context, user User, roleIDs []string) (User, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		roles, err := loadRoles(tx, roleIDs)
		if err != nil {
			return err
		}
		if err := tx.Create(&user).Error; err != nil {
			if errors.Is(err, gorm.ErrDuplicatedKey) {
				return ErrEmailExists
			}
			return err
		}
		if len(roles) > 0 {
			if err := tx.Model(&user).Association("Roles").Replace(&roles); err != nil {
				return err
			}
		}
		return nil
	})
	if err != nil {
		return User{}, err
	}
	return r.GetUser(ctx, user.ID)
}

func (r *Repository) CreateFirstOwner(
	ctx context.Context,
	user User,
	setups ...FirstOwnerSetup,
) (User, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		marker := BootstrapState{Key: "initial-owner"}
		if err := tx.Create(&marker).Error; err != nil {
			if errors.Is(err, gorm.ErrDuplicatedKey) {
				return ErrUsersExist
			}
			return err
		}

		var count int64
		if err := tx.Model(&User{}).Count(&count).Error; err != nil {
			return err
		}
		if count != 0 {
			return ErrUsersExist
		}

		var owner Role
		if err := tx.First(&owner, "id = ?", OwnerRoleID).Error; err != nil {
			return fmt.Errorf("load owner role: %w", err)
		}
		if err := tx.Create(&user).Error; err != nil {
			if errors.Is(err, gorm.ErrDuplicatedKey) {
				return ErrEmailExists
			}
			return err
		}
		if err := tx.Model(&user).Association("Roles").Append(&owner); err != nil {
			return err
		}
		for _, setup := range setups {
			if setup == nil {
				continue
			}
			if err := setup(ctx, tx, user); err != nil {
				return fmt.Errorf("set up first owner: %w", err)
			}
		}
		return nil
	})
	if err != nil {
		return User{}, err
	}
	return r.GetUser(ctx, user.ID)
}

func (r *Repository) UpdateUser(
	ctx context.Context,
	id string,
	changes UserChanges,
	passwordChangeSetups ...BeforePasswordChange,
) (User, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var user User
		if err := tx.Preload("Roles").First(&user, "id = ?", id).Error; err != nil {
			if errors.Is(err, gorm.ErrRecordNotFound) {
				return ErrUserNotFound
			}
			return err
		}

		if changes.Active != nil && !*changes.Active && user.Active && hasOwnerRole(user.Roles) {
			if err := ensureAnotherActiveOwner(tx, user.ID); err != nil {
				return err
			}
		}
		if changes.PasswordHash != nil {
			for _, setup := range passwordChangeSetups {
				if setup == nil {
					continue
				}
				if err := setup(ctx, tx, user, &changes); err != nil {
					return fmt.Errorf("prepare password change: %w", err)
				}
			}
		}

		updates := map[string]any{}
		if changes.Email != nil {
			updates["email"] = *changes.Email
		}
		if changes.DisplayName != nil {
			updates["display_name"] = *changes.DisplayName
		}
		if changes.PasswordHash != nil {
			updates["password_hash"] = *changes.PasswordHash
		}
		if changes.Active != nil {
			updates["active"] = *changes.Active
		}
		if len(updates) > 0 {
			if err := tx.Model(&user).Updates(updates).Error; err != nil {
				if errors.Is(err, gorm.ErrDuplicatedKey) {
					return ErrEmailExists
				}
				return err
			}
		}

		if changes.PasswordHash != nil || changes.Active != nil && !*changes.Active {
			if err := tx.Where("user_id = ?", user.ID).Delete(&Session{}).Error; err != nil {
				return err
			}
		}
		return nil
	}, &sql.TxOptions{Isolation: sql.LevelSerializable})
	if err != nil {
		return User{}, err
	}
	return r.GetUser(ctx, id)
}

func (r *Repository) ReplaceUserRoles(ctx context.Context, id string, roleIDs []string) (User, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var user User
		if err := tx.Preload("Roles").First(&user, "id = ?", id).Error; err != nil {
			if errors.Is(err, gorm.ErrRecordNotFound) {
				return ErrUserNotFound
			}
			return err
		}

		roles, err := loadRoles(tx, roleIDs)
		if err != nil {
			return err
		}
		if user.Active && hasOwnerRole(user.Roles) && !hasOwnerRole(roles) {
			if err := ensureAnotherActiveOwner(tx, user.ID); err != nil {
				return err
			}
		}
		if err := tx.Model(&user).Association("Roles").Replace(&roles); err != nil {
			return err
		}
		return tx.Model(&user).UpdateColumn("updated_at", time.Now().UTC()).Error
	}, &sql.TxOptions{Isolation: sql.LevelSerializable})
	if err != nil {
		return User{}, err
	}
	return r.GetUser(ctx, id)
}

func (r *Repository) GetRole(ctx context.Context, id string) (Role, error) {
	var role Role
	db := r.db.WithContext(ctx)
	err := preloadRole(db).First(&role, "id = ?", id).Error
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return Role{}, ErrRoleNotFound
	}
	if err == nil {
		role.CreatedByUser, err = loadCreator(db, role.CreatedByUserID)
	}
	return role, err
}

func (r *Repository) ListRoles(ctx context.Context) ([]Role, error) {
	var roles []Role
	db := r.db.WithContext(ctx)
	err := preloadRole(db).Order("normalized_name ASC").Find(&roles).Error
	if err == nil {
		err = hydrateRoleCreators(db, roles)
	}
	return roles, err
}

func (r *Repository) CreateRole(ctx context.Context, role Role, permissionKeys []string) (Role, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		permissions, err := loadPermissions(tx, permissionKeys)
		if err != nil {
			return err
		}
		if err := tx.Create(&role).Error; err != nil {
			if errors.Is(err, gorm.ErrDuplicatedKey) {
				return ErrRoleNameExists
			}
			return err
		}
		if len(permissions) > 0 {
			return tx.Model(&role).Association("Permissions").Replace(&permissions)
		}
		return nil
	})
	if err != nil {
		return Role{}, err
	}
	return r.GetRole(ctx, role.ID)
}

func (r *Repository) UpdateRole(ctx context.Context, id string, changes RoleChanges) (Role, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var role Role
		if err := tx.First(&role, "id = ?", id).Error; err != nil {
			if errors.Is(err, gorm.ErrRecordNotFound) {
				return ErrRoleNotFound
			}
			return err
		}
		if role.System {
			return ErrSystemRoleImmutable
		}

		updates := map[string]any{}
		if changes.Name != nil {
			updates["name"] = *changes.Name
		}
		if changes.NormalizedName != nil {
			updates["normalized_name"] = *changes.NormalizedName
		}
		if changes.Description != nil {
			updates["description"] = *changes.Description
		}
		if err := tx.Model(&role).Updates(updates).Error; err != nil {
			if errors.Is(err, gorm.ErrDuplicatedKey) {
				return ErrRoleNameExists
			}
			return err
		}
		return nil
	})
	if err != nil {
		return Role{}, err
	}
	return r.GetRole(ctx, id)
}

func (r *Repository) ReplaceRolePermissions(ctx context.Context, id string, keys []string) (Role, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var role Role
		if err := tx.First(&role, "id = ?", id).Error; err != nil {
			if errors.Is(err, gorm.ErrRecordNotFound) {
				return ErrRoleNotFound
			}
			return err
		}
		if role.System {
			return ErrSystemRoleImmutable
		}
		permissions, err := loadPermissions(tx, keys)
		if err != nil {
			return err
		}
		if err := tx.Model(&role).Association("Permissions").Replace(&permissions); err != nil {
			return err
		}
		return tx.Model(&role).UpdateColumn("updated_at", time.Now().UTC()).Error
	})
	if err != nil {
		return Role{}, err
	}
	return r.GetRole(ctx, id)
}

func (r *Repository) ListPermissions(ctx context.Context) ([]Permission, error) {
	var permissions []Permission
	err := r.db.WithContext(ctx).Order("key ASC").Find(&permissions).Error
	return permissions, err
}

func (r *Repository) CreateSession(ctx context.Context, session Session) error {
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := tx.Where("expires_at <= ?", session.CreatedAt).Delete(&Session{}).Error; err != nil {
			return err
		}
		return tx.Create(&session).Error
	})
}

func (r *Repository) FindSessionByHash(ctx context.Context, hash string) (Session, error) {
	var session Session
	err := r.db.WithContext(ctx).
		Preload("User.Roles.Permissions").
		Where("token_hash = ?", hash).
		First(&session).Error
	if err == nil {
		err = hydrateUserCreators(r.db.WithContext(ctx), &session.User)
	}
	return session, err
}

func (r *Repository) DeleteSessionByHash(ctx context.Context, hash string) error {
	return r.db.WithContext(ctx).Where("token_hash = ?", hash).Delete(&Session{}).Error
}

func (r *Repository) DeleteAllSessions(ctx context.Context) error {
	return r.db.WithContext(ctx).
		Session(&gorm.Session{AllowGlobalUpdate: true}).
		Delete(&Session{}).Error
}

func preloadUser(db *gorm.DB) *gorm.DB {
	return db.Preload("Roles.Permissions")
}

func preloadRole(db *gorm.DB) *gorm.DB {
	return db.Preload("Permissions")
}

func hydrateUsersCreators(db *gorm.DB, users []User) error {
	for index := range users {
		if err := hydrateUserCreators(db, &users[index]); err != nil {
			return err
		}
	}
	return nil
}

func hydrateUserCreators(db *gorm.DB, user *User) error {
	creator, err := loadCreator(db, user.CreatedByUserID)
	if err != nil {
		return err
	}
	user.CreatedByUser = creator
	return hydrateRoleCreators(db, user.Roles)
}

func hydrateRoleCreators(db *gorm.DB, roles []Role) error {
	for index := range roles {
		creator, err := loadCreator(db, roles[index].CreatedByUserID)
		if err != nil {
			return err
		}
		roles[index].CreatedByUser = creator
	}
	return nil
}

func loadCreator(db *gorm.DB, id *string) (*User, error) {
	if id == nil {
		return nil, nil
	}
	var creator User
	if err := db.Select("id", "email", "display_name").First(&creator, "id = ?", *id).Error; err != nil {
		return nil, err
	}
	return &creator, nil
}

func loadRoles(tx *gorm.DB, ids []string) ([]Role, error) {
	ids = uniqueStrings(ids)
	if len(ids) == 0 {
		return []Role{}, nil
	}
	var roles []Role
	if err := tx.Where("id IN ?", ids).Find(&roles).Error; err != nil {
		return nil, err
	}
	if len(roles) != len(ids) {
		return nil, ErrUnknownRole
	}
	return roles, nil
}

func loadPermissions(tx *gorm.DB, keys []string) ([]Permission, error) {
	keys = uniqueStrings(keys)
	if len(keys) == 0 {
		return []Permission{}, nil
	}
	var permissions []Permission
	if err := tx.Where("key IN ?", keys).Find(&permissions).Error; err != nil {
		return nil, err
	}
	if len(permissions) != len(keys) {
		return nil, ErrUnknownPermission
	}
	return permissions, nil
}

func ensureAnotherActiveOwner(tx *gorm.DB, excludedUserID string) error {
	var count int64
	err := tx.Table("users").
		Joins("JOIN user_roles ON user_roles.user_id = users.id").
		Where("user_roles.role_id = ? AND users.active = ? AND users.id <> ?", OwnerRoleID, true, excludedUserID).
		Count(&count).Error
	if err != nil {
		return err
	}
	if count == 0 {
		return ErrLastOwner
	}
	return nil
}

func hasOwnerRole(roles []Role) bool {
	for _, role := range roles {
		if role.ID == OwnerRoleID {
			return true
		}
	}
	return false
}

func uniqueStrings(values []string) []string {
	unique := make(map[string]struct{}, len(values))
	for _, value := range values {
		unique[value] = struct{}{}
	}
	result := make([]string, 0, len(unique))
	for value := range unique {
		result = append(result, value)
	}
	sort.Strings(result)
	return result
}
