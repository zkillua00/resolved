package requestproxy

import (
	"context"

	"resolved-server/internal/identity"
	"resolved-server/internal/resourceevents"
)

func (s *Service) ListProxies(ctx context.Context) ([]Proxy, error) {
	return s.proxies.List(ctx)
}

func (s *Service) CreateProxy(
	ctx context.Context,
	actorUserID string,
	name string,
	rules []HostnameOverride,
) (Proxy, error) {
	proxy, err := s.proxies.Create(ctx, &actorUserID, name, rules)
	if err != nil {
		return Proxy{}, err
	}
	s.emitProxyChange(resourceevents.ActionCreated, actorUserID, proxy, []resourceevents.Diff{
		{Field: "rule_count", From: 0, To: len(proxy.Rules)},
	})
	return proxy, nil
}

func (s *Service) UpdateProxy(
	ctx context.Context,
	actorUserID string,
	proxyID string,
	name *string,
	rules *[]HostnameOverride,
) (Proxy, error) {
	before, err := s.proxies.Get(ctx, proxyID)
	if err != nil {
		return Proxy{}, err
	}
	proxy, err := s.proxies.Update(ctx, proxyID, name, rules)
	if err != nil {
		return Proxy{}, err
	}
	// Rules may have changed; drop pooled connections keyed on the previous
	// dial targets.
	s.client.CloseIdleConnections()
	s.emitProxyChange(resourceevents.ActionUpdated, actorUserID, proxy, []resourceevents.Diff{
		{Field: "name", From: before.Name, To: proxy.Name},
		{Field: "rule_count", From: len(before.Rules), To: len(proxy.Rules)},
	})
	return proxy, nil
}

func (s *Service) DeleteProxy(ctx context.Context, actorUserID, proxyID string) (Proxy, error) {
	proxy, err := s.proxies.Delete(ctx, proxyID)
	if err != nil {
		return Proxy{}, err
	}
	s.client.CloseIdleConnections()
	s.emitProxyChange(resourceevents.ActionDeleted, actorUserID, proxy, nil)
	return proxy, nil
}

func (s *Service) ReplaceProxyAssignments(
	ctx context.Context,
	actorUserID string,
	proxyID string,
	assignments []ProxyAssignment,
) (Proxy, error) {
	before, err := s.proxies.Get(ctx, proxyID)
	if err != nil {
		return Proxy{}, err
	}
	proxy, err := s.proxies.ReplaceAssignments(ctx, proxyID, assignments)
	if err != nil {
		return Proxy{}, err
	}
	s.client.CloseIdleConnections()
	s.emitProxyChange(resourceevents.ActionUpdated, actorUserID, proxy, []resourceevents.Diff{
		{Field: "assignment_count", From: len(before.Assignments), To: len(proxy.Assignments)},
	})
	return proxy, nil
}

func (s *Service) ReplaceProxyExclusions(
	ctx context.Context,
	actorUserID string,
	proxyID string,
	excludedUserIDs []string,
	excludedRoleIDs []string,
) (Proxy, error) {
	before, err := s.proxies.Get(ctx, proxyID)
	if err != nil {
		return Proxy{}, err
	}
	proxy, err := s.proxies.ReplaceExclusions(ctx, proxyID, excludedUserIDs, excludedRoleIDs)
	if err != nil {
		return Proxy{}, err
	}
	s.client.CloseIdleConnections()
	s.emitProxyChange(resourceevents.ActionUpdated, actorUserID, proxy, []resourceevents.Diff{
		{
			Field: "exclusion_count",
			From:  len(before.ExcludedUserIDs) + len(before.ExcludedRoleIDs),
			To:    len(proxy.ExcludedUserIDs) + len(proxy.ExcludedRoleIDs),
		},
	})
	return proxy, nil
}

func (s *Service) emitProxyChange(
	action resourceevents.Action,
	actorUserID string,
	proxy Proxy,
	diffs []resourceevents.Diff,
) {
	resourceevents.Emit(s.events, resourceevents.Change{
		Resource:    resourceevents.ResourceProxy,
		Action:      action,
		ResourceID:  proxy.ID,
		ActorUserID: actorUserID,
		TargetName:  proxy.Name,
		Audience: resourceevents.Audience{
			PermissionKeys: []string{identity.PermissionProxiesRead},
		},
		Diffs: diffs,
	})
}
