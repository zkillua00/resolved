package identity

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"strings"
	"time"

	"resolved-server/internal/security"

	"gorm.io/gorm"
)

type Repository struct {
	db         *gorm.DB
	dataCipher *security.DataCipher
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

func NewRepository(db *gorm.DB, dataCipher ...*security.DataCipher) *Repository {
	repository := &Repository{db: db}
	if len(dataCipher) > 0 {
		repository.dataCipher = dataCipher[0]
	}
	return repository
}

func (r *Repository) FindUserByEmail(ctx context.Context, email string) (User, error) {
	var user User
	db := r.db.WithContext(ctx)
	query := db.Where("email = ?", email)
	if r.dataCipher != nil {
		lookup, err := r.dataCipher.LookupDigest(
			ctx, db, security.DeploymentDataScope(), "user_email", strings.ToLower(strings.TrimSpace(email)),
		)
		if err != nil {
			return User{}, fmt.Errorf("compute user login lookup: %w", err)
		}
		query = db.Where("email_lookup = ?", lookup)
	}
	err := query.First(&user).Error
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return User{}, ErrUserNotFound
	}
	if err == nil {
		err = DecryptUserProfile(ctx, db, r.dataCipher, &user)
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
		err = r.hydrateUserCreators(db, &user)
	}
	return user, err
}

func (r *Repository) ListUsers(ctx context.Context) ([]User, error) {
	var users []User
	db := r.db.WithContext(ctx)
	err := preloadUser(db).Order("id ASC").Find(&users).Error
	if err == nil {
		err = r.hydrateUsersCreators(db, users)
	}
	if err == nil {
		sort.Slice(users, func(left, right int) bool {
			return users[left].Email < users[right].Email
		})
	}
	return users, err
}

func (r *Repository) CreateUser(ctx context.Context, user User, roleIDs []string) (User, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		roles, err := loadRoles(tx, roleIDs)
		if err != nil {
			return err
		}
		storedUser := user
		if err := r.encryptUserProfile(ctx, tx, &storedUser); err != nil {
			return err
		}
		if err := tx.Create(&storedUser).Error; err != nil {
			if errors.Is(err, gorm.ErrDuplicatedKey) {
				return ErrEmailExists
			}
			return err
		}
		if len(roles) > 0 {
			if err := tx.Model(&storedUser).Association("Roles").Replace(&roles); err != nil {
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
		storedUser := user
		if err := r.encryptUserProfile(ctx, tx, &storedUser); err != nil {
			return err
		}
		if err := tx.Create(&storedUser).Error; err != nil {
			if errors.Is(err, gorm.ErrDuplicatedKey) {
				return ErrEmailExists
			}
			return err
		}
		if err := tx.Model(&storedUser).Association("Roles").Append(&owner); err != nil {
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
		if err := DecryptUserProfile(ctx, tx, r.dataCipher, &user); err != nil {
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
		if changes.Email != nil || changes.DisplayName != nil {
			if r.dataCipher == nil {
				if changes.Email != nil {
					updates["email"] = *changes.Email
				}
				if changes.DisplayName != nil {
					updates["display_name"] = *changes.DisplayName
				}
			} else {
				profile := User{ID: user.ID, Email: user.Email, DisplayName: user.DisplayName}
				if changes.Email != nil {
					profile.Email = *changes.Email
				}
				if changes.DisplayName != nil {
					profile.DisplayName = *changes.DisplayName
				}
				if err := r.encryptUserProfile(ctx, tx, &profile); err != nil {
					return err
				}
				updates["email"] = ""
				updates["display_name"] = ""
				updates["email_lookup"] = profile.EmailLookup
				updates["encrypted_profile"] = profile.EncryptedProfile
			}
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
		err = r.decryptRoleProfile(ctx, db, &role)
	}
	if err == nil {
		role.CreatedByUser, err = r.loadCreator(db, role.CreatedByUserID)
	}
	return role, err
}

func (r *Repository) ListRoles(ctx context.Context) ([]Role, error) {
	var roles []Role
	db := r.db.WithContext(ctx)
	err := preloadRole(db).Order("id ASC").Find(&roles).Error
	if err == nil {
		err = r.hydrateRoleCreators(db, roles)
	}
	if err == nil {
		sort.Slice(roles, func(left, right int) bool {
			return roles[left].NormalizedName < roles[right].NormalizedName
		})
	}
	return roles, err
}

func (r *Repository) CreateRole(ctx context.Context, role Role, permissionKeys []string) (Role, error) {
	err := r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		permissions, err := loadPermissions(tx, permissionKeys)
		if err != nil {
			return err
		}
		storedRole := role
		if err := r.encryptRoleProfile(ctx, tx, &storedRole); err != nil {
			return err
		}
		if err := tx.Create(&storedRole).Error; err != nil {
			if errors.Is(err, gorm.ErrDuplicatedKey) {
				return ErrRoleNameExists
			}
			return err
		}
		if len(permissions) > 0 {
			return tx.Model(&storedRole).Association("Permissions").Replace(&permissions)
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
		if r.dataCipher == nil {
			if changes.Name != nil {
				updates["name"] = *changes.Name
			}
			if changes.NormalizedName != nil {
				updates["normalized_name"] = *changes.NormalizedName
			}
			if changes.Description != nil {
				updates["description"] = *changes.Description
			}
		} else if changes.Name != nil || changes.NormalizedName != nil || changes.Description != nil {
			if err := r.decryptRoleProfile(ctx, tx, &role); err != nil {
				return err
			}
			profile := Role{
				ID: role.ID, Name: role.Name, NormalizedName: role.NormalizedName, Description: role.Description,
			}
			if changes.Name != nil {
				profile.Name = *changes.Name
			}
			if changes.NormalizedName != nil {
				profile.NormalizedName = *changes.NormalizedName
			}
			if changes.Description != nil {
				profile.Description = *changes.Description
			}
			if err := r.encryptRoleProfile(ctx, tx, &profile); err != nil {
				return err
			}
			updates["name"] = ""
			updates["normalized_name"] = ""
			updates["description"] = ""
			updates["normalized_name_lookup"] = profile.NormalizedNameLookup
			updates["encrypted_profile"] = profile.EncryptedProfile
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
		err = r.hydrateUserCreators(r.db.WithContext(ctx), &session.User)
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

func (r *Repository) hydrateUsersCreators(db *gorm.DB, users []User) error {
	for index := range users {
		if err := r.hydrateUserCreators(db, &users[index]); err != nil {
			return err
		}
	}
	return nil
}

func (r *Repository) hydrateUserCreators(db *gorm.DB, user *User) error {
	if err := DecryptUserProfile(db.Statement.Context, db, r.dataCipher, user); err != nil {
		return err
	}
	creator, err := r.loadCreator(db, user.CreatedByUserID)
	if err != nil {
		return err
	}
	user.CreatedByUser = creator
	return r.hydrateRoleCreators(db, user.Roles)
}

func (r *Repository) hydrateRoleCreators(db *gorm.DB, roles []Role) error {
	for index := range roles {
		if err := r.decryptRoleProfile(db.Statement.Context, db, &roles[index]); err != nil {
			return err
		}
		creator, err := r.loadCreator(db, roles[index].CreatedByUserID)
		if err != nil {
			return err
		}
		roles[index].CreatedByUser = creator
	}
	return nil
}

func (r *Repository) loadCreator(db *gorm.DB, id *string) (*User, error) {
	if id == nil {
		return nil, nil
	}
	var creator User
	if err := db.Select("id", "email", "display_name", "encrypted_profile").First(&creator, "id = ?", *id).Error; err != nil {
		return nil, err
	}
	if err := DecryptUserProfile(db.Statement.Context, db, r.dataCipher, &creator); err != nil {
		return nil, err
	}
	return &creator, nil
}

type encryptedUserProfile struct {
	Email       string `json:"email"`
	DisplayName string `json:"display_name"`
}

type encryptedRoleProfile struct {
	Name           string `json:"name"`
	NormalizedName string `json:"normalized_name"`
	Description    string `json:"description"`
}

func (r *Repository) encryptRoleProfile(ctx context.Context, db *gorm.DB, role *Role) error {
	if r.dataCipher == nil {
		return nil
	}
	lookup, err := r.dataCipher.LookupDigest(
		ctx, db, security.DeploymentDataScope(), "role_name", role.NormalizedName,
	)
	if err != nil {
		return fmt.Errorf("compute role-name lookup: %w", err)
	}
	plaintext, err := json.Marshal(encryptedRoleProfile{
		Name: role.Name, NormalizedName: role.NormalizedName, Description: role.Description,
	})
	if err != nil {
		return fmt.Errorf("encode role profile encryption payload: %w", err)
	}
	defer clear(plaintext)
	role.EncryptedProfile, err = r.dataCipher.Encrypt(
		ctx, db, security.DeploymentDataScope(), "role_profile", role.ID, plaintext,
	)
	if err != nil {
		return fmt.Errorf("encrypt role profile: %w", err)
	}
	role.NormalizedNameLookup = &lookup
	role.Name = ""
	role.NormalizedName = ""
	role.Description = ""
	return nil
}

func (r *Repository) decryptRoleProfile(ctx context.Context, db *gorm.DB, role *Role) error {
	if len(role.EncryptedProfile) == 0 {
		return nil
	}
	if r.dataCipher == nil {
		return security.ErrDataKeyUnavailable
	}
	plaintext, err := r.dataCipher.Decrypt(
		ctx, db, security.DeploymentDataScope(), "role_profile", role.ID, role.EncryptedProfile,
	)
	if err != nil {
		return fmt.Errorf("decrypt role profile: %w", err)
	}
	defer clear(plaintext)
	var profile encryptedRoleProfile
	if err := json.Unmarshal(plaintext, &profile); err != nil {
		return fmt.Errorf("decode role profile encryption payload: %w", err)
	}
	role.Name = profile.Name
	role.NormalizedName = profile.NormalizedName
	role.Description = profile.Description
	return nil
}

func (r *Repository) EncryptLegacyRoles(ctx context.Context) error {
	if r.dataCipher == nil {
		return security.ErrDataKeyUnavailable
	}
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var roles []Role
		if err := tx.Where(
			"encrypted_profile IS NULL OR name <> ? OR normalized_name <> ? OR description <> ?",
			"", "", "",
		).Find(&roles).Error; err != nil {
			return err
		}
		for index := range roles {
			if len(roles[index].EncryptedProfile) > 0 {
				if err := r.decryptRoleProfile(ctx, tx, &roles[index]); err != nil {
					return err
				}
			}
			// Seeded system-role metadata is authoritative on every startup.
			if roles[index].ID == OwnerRoleID {
				roles[index].Name = "Owner"
				roles[index].NormalizedName = "owner"
				roles[index].Description = "Built-in deployment owner with every permission"
			}
			if err := r.encryptRoleProfile(ctx, tx, &roles[index]); err != nil {
				return err
			}
			if err := tx.Model(&Role{}).Where("id = ?", roles[index].ID).Updates(map[string]any{
				"name": "", "normalized_name": "", "description": "",
				"normalized_name_lookup": roles[index].NormalizedNameLookup,
				"encrypted_profile":      roles[index].EncryptedProfile,
			}).Error; err != nil {
				if errors.Is(err, gorm.ErrDuplicatedKey) {
					return ErrRoleNameExists
				}
				return err
			}
		}
		return nil
	})
}

func (r *Repository) encryptUserProfile(ctx context.Context, db *gorm.DB, user *User) error {
	if r.dataCipher == nil {
		return nil
	}
	lookup, err := r.dataCipher.LookupDigest(
		ctx, db, security.DeploymentDataScope(), "user_email", strings.ToLower(strings.TrimSpace(user.Email)),
	)
	if err != nil {
		return fmt.Errorf("compute user email lookup: %w", err)
	}
	plaintext, err := json.Marshal(encryptedUserProfile{Email: user.Email, DisplayName: user.DisplayName})
	if err != nil {
		return fmt.Errorf("encode user profile encryption payload: %w", err)
	}
	defer clear(plaintext)
	user.EncryptedProfile, err = r.dataCipher.Encrypt(
		ctx, db, security.DeploymentDataScope(), "user_profile", user.ID, plaintext,
	)
	if err != nil {
		return fmt.Errorf("encrypt user profile: %w", err)
	}
	user.EmailLookup = &lookup
	user.Email = ""
	user.DisplayName = ""
	return nil
}

func DecryptUserProfile(
	ctx context.Context,
	db *gorm.DB,
	dataCipher *security.DataCipher,
	user *User,
) error {
	if len(user.EncryptedProfile) == 0 {
		return nil
	}
	if dataCipher == nil {
		return security.ErrDataKeyUnavailable
	}
	plaintext, err := dataCipher.Decrypt(
		ctx, db, security.DeploymentDataScope(), "user_profile", user.ID, user.EncryptedProfile,
	)
	if err != nil {
		return fmt.Errorf("decrypt user profile: %w", err)
	}
	defer clear(plaintext)
	var profile encryptedUserProfile
	if err := json.Unmarshal(plaintext, &profile); err != nil {
		return fmt.Errorf("decode user profile encryption payload: %w", err)
	}
	user.Email = profile.Email
	user.DisplayName = profile.DisplayName
	return nil
}

func (r *Repository) EncryptLegacyUsers(ctx context.Context) error {
	if r.dataCipher == nil {
		return security.ErrDataKeyUnavailable
	}
	return r.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		var users []User
		if err := tx.Where("encrypted_profile IS NULL").Find(&users).Error; err != nil {
			return err
		}
		for index := range users {
			if err := r.encryptUserProfile(ctx, tx, &users[index]); err != nil {
				return err
			}
			if err := tx.Model(&User{}).Where("id = ?", users[index].ID).Updates(map[string]any{
				"email": "", "display_name": "", "email_lookup": users[index].EmailLookup,
				"encrypted_profile": users[index].EncryptedProfile,
			}).Error; err != nil {
				if errors.Is(err, gorm.ErrDuplicatedKey) {
					return ErrEmailExists
				}
				return err
			}
		}
		return nil
	})
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
