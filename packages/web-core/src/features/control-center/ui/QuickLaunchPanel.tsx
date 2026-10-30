import { useEffect, useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import {
  CheckCircleIcon,
  CircleIcon,
  RocketLaunchIcon,
  SpinnerIcon,
  XCircleIcon,
} from '@phosphor-icons/react';
import type { ExecutorConfig } from 'shared/types';
import { repoApi } from '@/shared/lib/api';
import { useRepoBranches } from '@/shared/hooks/useRepoBranches';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { cn } from '@/shared/lib/utils';
import { parseTaskPrompts } from '../model/parseTaskPrompts';
import {
  useQuickLaunch,
  type LaunchStatus,
  type LaunchTarget,
} from '../model/useQuickLaunch';
import { Panel } from './Panel';

const MAX_TASKS = 20;

const fieldClassName =
  'w-full rounded-sm border border-border bg-primary px-base py-half text-base text-normal focus:outline-none focus:ring-1 focus:ring-brand disabled:opacity-60';

function LaunchStatusIcon({ status }: { status: LaunchStatus }) {
  switch (status) {
    case 'launching':
      return (
        <SpinnerIcon className="size-icon-sm shrink-0 animate-spin text-low" />
      );
    case 'launched':
      return (
        <CheckCircleIcon
          className="size-icon-sm shrink-0 text-success"
          weight="fill"
        />
      );
    case 'failed':
      return (
        <XCircleIcon
          className="size-icon-sm shrink-0 text-error"
          weight="fill"
        />
      );
    default:
      return <CircleIcon className="size-icon-sm shrink-0 text-low" />;
  }
}

function formatExecutor(config: ExecutorConfig | null): string {
  if (!config) return 'No default agent configured';
  const name = config.executor
    .toLowerCase()
    .split('_')
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ');
  return config.variant && config.variant !== 'DEFAULT'
    ? `${name} · ${config.variant}`
    : name;
}

export function QuickLaunchPanel({
  onOpenWorkspace,
}: {
  onOpenWorkspace: (workspaceId: string) => void;
}) {
  const { config } = useUserSystem();
  const { tasks, isLaunching, launch, retryFailed, clear } = useQuickLaunch();

  const reposQuery = useQuery({
    queryKey: ['control-center', 'repos'],
    queryFn: () => repoApi.list(),
    staleTime: 30_000,
  });
  const repos = useMemo(() => reposQuery.data ?? [], [reposQuery.data]);

  const [repoId, setRepoId] = useState<string>('');
  const [branch, setBranch] = useState<string>('');
  const [text, setText] = useState('');

  useEffect(() => {
    if (!repoId && repos.length > 0) setRepoId(repos[0].id);
  }, [repoId, repos]);

  const branchesQuery = useRepoBranches(repoId || null);
  const localBranches = useMemo(
    () => (branchesQuery.data ?? []).filter((b) => !b.is_remote),
    [branchesQuery.data]
  );

  useEffect(() => {
    setBranch('');
  }, [repoId]);

  useEffect(() => {
    if (branch || localBranches.length === 0) return;
    const repo = repos.find((r) => r.id === repoId);
    const preferred =
      localBranches.find((b) => b.name === repo?.default_target_branch) ??
      localBranches.find((b) => b.is_current) ??
      localBranches[0];
    setBranch(preferred.name);
  }, [branch, localBranches, repoId, repos]);

  const executorConfig: ExecutorConfig | null = config?.executor_profile
    ? {
        executor: config.executor_profile.executor,
        variant: config.executor_profile.variant ?? null,
      }
    : null;

  const prompts = useMemo(() => parseTaskPrompts(text), [text]);
  const tooMany = prompts.length > MAX_TASKS;
  const target: LaunchTarget | null =
    repoId && branch && executorConfig
      ? { repoId, targetBranch: branch, executorConfig }
      : null;
  const canLaunch = !!target && prompts.length > 0 && !tooMany && !isLaunching;
  const failedCount = tasks.filter((t) => t.status === 'failed').length;
  const launchedCount = tasks.filter((t) => t.status === 'launched').length;

  const handleLaunch = async () => {
    if (!target || !canLaunch) return;
    await launch(prompts, target);
    setText('');
  };

  return (
    <Panel title="Quick launch">
      <div className="flex flex-col gap-base">
        <div className="grid grid-cols-2 gap-base">
          <label className="flex flex-col gap-half text-sm text-low">
            Repository
            <select
              className={fieldClassName}
              value={repoId}
              onChange={(e) => setRepoId(e.target.value)}
              disabled={isLaunching || repos.length === 0}
            >
              {repos.length === 0 && (
                <option value="">
                  {reposQuery.isLoading ? 'Loading…' : 'No repositories'}
                </option>
              )}
              {repos.map((repo) => (
                <option key={repo.id} value={repo.id}>
                  {repo.display_name}
                </option>
              ))}
            </select>
          </label>
          <label className="flex flex-col gap-half text-sm text-low">
            Base branch
            <select
              className={cn(fieldClassName, 'font-ibm-plex-mono')}
              value={branch}
              onChange={(e) => setBranch(e.target.value)}
              disabled={isLaunching || localBranches.length === 0}
            >
              {localBranches.length === 0 && (
                <option value="">
                  {branchesQuery.isLoading ? 'Loading…' : 'No branches'}
                </option>
              )}
              {localBranches.map((b) => (
                <option key={b.name} value={b.name}>
                  {b.name}
                </option>
              ))}
            </select>
          </label>
        </div>

        <label className="flex flex-col gap-half text-sm text-low">
          Task prompts
          <textarea
            className={cn(fieldClassName, 'min-h-32 resize-y')}
            value={text}
            onChange={(e) => setText(e.target.value)}
            disabled={isLaunching}
            placeholder={
              'One task per line, or separate multi-line prompts with a blank line.'
            }
          />
        </label>

        <div className="flex items-center gap-base text-sm text-low">
          <span className="min-w-0 flex-1 truncate">
            Agent: {formatExecutor(executorConfig)}
          </span>
          <span className={cn('tabular-nums', tooMany && 'text-error')}>
            {tooMany
              ? `Max ${MAX_TASKS} tasks`
              : `${prompts.length} task${prompts.length === 1 ? '' : 's'}`}
          </span>
          <button
            type="button"
            onClick={handleLaunch}
            disabled={!canLaunch}
            className={cn(
              'flex items-center gap-half rounded-sm px-base py-half text-base',
              'focus:outline-none focus-visible:ring-1 focus-visible:ring-brand',
              canLaunch
                ? 'cursor-pointer bg-brand text-on-brand hover:bg-brand-hover'
                : 'cursor-not-allowed bg-panel text-low'
            )}
          >
            {isLaunching ? (
              <SpinnerIcon className="size-icon-sm animate-spin" />
            ) : (
              <RocketLaunchIcon className="size-icon-sm" weight="bold" />
            )}
            Launch
          </button>
        </div>

        {tasks.length > 0 && (
          <div className="flex flex-col gap-half border-t border-border pt-base">
            <div className="flex items-center gap-base text-sm text-low">
              <span className="tabular-nums">
                {launchedCount} of {tasks.length} launched
                {failedCount > 0 && `, ${failedCount} failed`}
              </span>
              {!isLaunching && (
                <span className="ml-auto flex items-center gap-base">
                  {failedCount > 0 && (
                    <button
                      type="button"
                      onClick={() => void retryFailed()}
                      className="cursor-pointer text-normal hover:text-high"
                    >
                      Retry failed
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={clear}
                    className="cursor-pointer text-low hover:text-normal"
                  >
                    Clear
                  </button>
                </span>
              )}
            </div>
            <ul className="flex max-h-64 flex-col gap-half overflow-y-auto">
              {tasks.map((task) => (
                <li key={task.id} className="flex items-start gap-half text-sm">
                  <LaunchStatusIcon status={task.status} />
                  <div className="min-w-0 flex-1">
                    {task.workspaceId ? (
                      <button
                        type="button"
                        onClick={() => {
                          if (task.workspaceId)
                            onOpenWorkspace(task.workspaceId);
                        }}
                        className="block max-w-full cursor-pointer truncate text-left text-normal hover:text-high hover:underline"
                      >
                        {task.title}
                      </button>
                    ) : (
                      <span className="block truncate text-normal">
                        {task.title}
                      </span>
                    )}
                    {task.error && (
                      <span className="block text-error">{task.error}</span>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </Panel>
  );
}
