import type { CreateModeInitialState } from '@/shared/types/createMode';
import type { DraftWorkspaceData } from 'shared/types';
import { ScratchType } from 'shared/types';
import type { AppRuntime } from '@/shared/hooks/useAppRuntime';
import { scratchApi } from '@/shared/lib/api';
import { localStorageScratchUpdate } from '@/shared/hooks/useLocalStorageScratch';

interface WorkspaceDefaultsLike {
  preferredRepos?: CreateModeInitialState['preferredRepos'];
  project_id?: string | null;
}

interface LocalWorkspaceLike {
  id: string;
}

interface LinkedIssueSource {
  id: string;
  simple_id: string;
  title: string;
}

/** The single draft id every tab shared before drafts became per tab. */
const LEGACY_SHARED_DRAFT_ID = '00000000-0000-0000-0000-000000000001';
const TAB_DRAFT_ID_KEY = 'jarvis.createDraftId';
const LEGACY_DRAFT_ADOPTED_KEY = 'jarvis.legacyCreateDraftAdopted';
const LAST_CREATE_REPOS_KEY = 'jarvis.lastCreateRepos';

/**
 * Draft id for this tab's /workspaces/create form. Kept in sessionStorage so
 * each tab prepares its own launch and keeps it across reloads. The first tab
 * after the upgrade adopts the old shared draft so nothing in progress is lost.
 */
export function getTabWorkspaceCreateDraftId(): string {
  try {
    const existing = sessionStorage.getItem(TAB_DRAFT_ID_KEY);
    if (existing) return existing;

    let id: string;
    if (localStorage.getItem(LEGACY_DRAFT_ADOPTED_KEY)) {
      id = crypto.randomUUID();
    } else {
      localStorage.setItem(LEGACY_DRAFT_ADOPTED_KEY, '1');
      id = LEGACY_SHARED_DRAFT_ID;
    }
    sessionStorage.setItem(TAB_DRAFT_ID_KEY, id);
    return id;
  } catch {
    return LEGACY_SHARED_DRAFT_ID;
  }
}

type LastCreateRepos = NonNullable<CreateModeInitialState['preferredRepos']>;

/** Remember the repos picked in the create form to seed new tabs' drafts. */
export function rememberLastCreateRepos(repos: LastCreateRepos): void {
  if (repos.length === 0) return;
  try {
    localStorage.setItem(LAST_CREATE_REPOS_KEY, JSON.stringify(repos));
  } catch {
    // Seeding new tabs is best effort.
  }
}

export function getLastCreateRepos(): LastCreateRepos | null {
  try {
    const raw = localStorage.getItem(LAST_CREATE_REPOS_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return null;
    const repos = parsed.filter(
      (repo): repo is LastCreateRepos[number] =>
        typeof repo?.repo_id === 'string' &&
        (repo.target_branch === null || typeof repo.target_branch === 'string')
    );
    return repos.length > 0 ? repos : null;
  } catch {
    return null;
  }
}

export function buildWorkspaceCreatePrompt(
  title: string | null | undefined,
  description: string | null | undefined
): string | null {
  const trimmedTitle = title?.trim();
  if (!trimmedTitle) return null;

  const trimmedDescription = description?.trim();
  return trimmedDescription
    ? `${trimmedTitle}\n\n${trimmedDescription}`
    : trimmedTitle;
}

export function buildLinkedIssueCreateState(
  issue: LinkedIssueSource | null | undefined,
  projectId: string
): NonNullable<CreateModeInitialState['linkedIssue']> | null {
  if (!issue) return null;
  return {
    issueId: issue.id,
    simpleId: issue.simple_id,
    title: issue.title,
    remoteProjectId: projectId,
  };
}

export function buildWorkspaceCreateInitialState(args: {
  prompt: string | null;
  defaults?: WorkspaceDefaultsLike | null;
  linkedIssue?: CreateModeInitialState['linkedIssue'];
  executorConfig?: CreateModeInitialState['executorConfig'];
}): CreateModeInitialState {
  return {
    initialPrompt: args.prompt,
    preferredRepos: args.defaults?.preferredRepos ?? null,
    project_id: args.defaults?.project_id ?? null,
    linkedIssue: args.linkedIssue ?? null,
    executorConfig: args.executorConfig ?? null,
  };
}

export function buildLocalWorkspaceIdSet(
  activeWorkspaces: LocalWorkspaceLike[],
  archivedWorkspaces: LocalWorkspaceLike[]
): Set<string> {
  return new Set([
    ...activeWorkspaces.map((workspace) => workspace.id),
    ...archivedWorkspaces.map((workspace) => workspace.id),
  ]);
}

export function toDraftWorkspaceData(
  initialState: CreateModeInitialState
): DraftWorkspaceData {
  return {
    message: initialState.initialPrompt ?? '',
    repos:
      initialState.preferredRepos?.map((repo) => ({
        repo_id: repo.repo_id,
        target_branch: repo.target_branch ?? '',
      })) ?? [],
    executor_config: initialState.executorConfig ?? null,
    linked_issue: initialState.linkedIssue
      ? {
          issue_id: initialState.linkedIssue.issueId,
          simple_id: initialState.linkedIssue.simpleId ?? '',
          title: initialState.linkedIssue.title ?? '',
          remote_project_id: initialState.linkedIssue.remoteProjectId,
        }
      : null,
    attachments: [],
  };
}

export async function persistWorkspaceCreateDraft(
  initialState: CreateModeInitialState,
  draftId: string,
  runtime: AppRuntime = 'local'
): Promise<string | null> {
  const draftData = toDraftWorkspaceData(initialState);
  const payload = {
    type: 'DRAFT_WORKSPACE' as const,
    data: draftData,
  };

  try {
    if (runtime === 'remote') {
      const didPersist = localStorageScratchUpdate(
        ScratchType.DRAFT_WORKSPACE,
        draftId,
        {
          payload,
        }
      );
      if (!didPersist) {
        throw new Error('Failed to persist create-workspace draft in storage');
      }
    } else {
      await scratchApi.update(ScratchType.DRAFT_WORKSPACE, draftId, {
        payload,
      });
    }
    return draftId;
  } catch (error) {
    console.error('Failed to persist create-workspace draft:', error);
    return null;
  }
}
