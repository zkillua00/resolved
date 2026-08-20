package activitylog

import (
	"context"
	"encoding/json"

	"resolved-server/internal/problem"
	"resolved-server/internal/workspaces"
)

type Service struct {
	repository *Repository
	workspaces *workspaces.Service
}

func NewService(repository *Repository, workspaceService *workspaces.Service) *Service {
	return &Service{repository: repository, workspaces: workspaceService}
}

func (s *Service) ListWorkspace(
	ctx context.Context,
	actor workspaces.Actor,
	workspaceID string,
	input ListInput,
) (PageView, error) {
	page, err := pageQueryFromInput(input)
	if err != nil {
		return PageView{}, err
	}
	scope, err := s.workspaces.ResolveAccessScope(ctx, actor, workspaceID)
	if err != nil {
		return PageView{}, err
	}
	entries, err := s.repository.ListWorkspace(
		ctx,
		workspaceID,
		scope.AllCollections,
		scope.CollectionIDs,
		page,
	)
	if err != nil {
		return PageView{}, problem.Wrap(err, "list workspace change log")
	}
	return viewPage(entries, input)
}

func (s *Service) ListAudit(ctx context.Context, input ListInput) (PageView, error) {
	page, err := pageQueryFromInput(input)
	if err != nil {
		return PageView{}, err
	}
	entries, err := s.repository.ListAudit(ctx, page)
	if err != nil {
		return PageView{}, problem.Wrap(err, "list identity audit log")
	}
	return viewPage(entries, input)
}

func pageQueryFromInput(input ListInput) (pageQuery, error) {
	if input.Cursor != "" && input.After != "" {
		return pageQuery{}, problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"cursor": "cannot be combined with after"},
		)
	}
	before, err := decodeCursor("cursor", input.Cursor)
	if err != nil {
		return pageQuery{}, err
	}
	after, err := decodeCursor("after", input.After)
	if err != nil {
		return pageQuery{}, err
	}
	limit := input.Limit
	if limit == 0 {
		limit = DefaultPageLimit
	}
	if limit < 1 || limit > MaxPageLimit {
		return pageQuery{}, problem.WithFields(
			"validation_failed",
			"request validation failed",
			map[string]string{"limit": "must be between 1 and 100"},
		)
	}
	return pageQuery{Before: before, After: after, Limit: limit}, nil
}

func viewPage(page entryPage, input ListInput) (PageView, error) {
	entries, err := viewEntries(page.Entries)
	if err != nil {
		return PageView{}, err
	}
	view := PageView{Entries: entries, HasMoreNewer: input.After != "" && page.HasMore}
	if input.After != "" {
		cursor := input.After
		if len(page.Entries) > 0 {
			cursor = encodeCursor(page.Entries[len(page.Entries)-1])
		}
		view.NewerCursor = &cursor
		return view, nil
	}
	if input.Cursor == "" && len(page.Entries) > 0 {
		cursor := encodeCursor(page.Entries[0])
		view.NewerCursor = &cursor
	}
	if page.HasMore && len(page.Entries) > 0 {
		cursor := encodeCursor(page.Entries[len(page.Entries)-1])
		view.OlderCursor = &cursor
	}
	return view, nil
}

func viewEntries(entries []Entry) ([]EntryView, error) {
	views := make([]EntryView, 0, len(entries))
	for _, entry := range entries {
		var diffs []DiffView
		if err := json.Unmarshal(entry.DiffsJSON, &diffs); err != nil {
			return nil, problem.Wrap(err, "decode activity log diff")
		}
		if diffs == nil {
			diffs = []DiffView{}
		}
		views = append(views, EntryView{
			ID: entry.ID, Kind: entry.Kind, Resource: entry.Resource, Action: entry.Action,
			ResourceID: entry.ResourceID, WorkspaceID: entry.WorkspaceID,
			CollectionID: entry.CollectionID, ActorUserID: entry.ActorUserID,
			ActorEmail: entry.ActorEmail, ActorDisplayName: entry.ActorDisplayName,
			TargetName: entry.TargetName, Diffs: diffs, CreatedAt: entry.CreatedAt,
		})
	}
	return views, nil
}
