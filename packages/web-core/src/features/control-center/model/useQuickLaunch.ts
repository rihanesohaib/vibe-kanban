import { useCallback, useRef, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type { ExecutorConfig } from 'shared/types';
import { workspacesApi } from '@/shared/lib/api';
import { splitMessageToTitleDescription } from '@/shared/lib/string';
import { workspaceSummaryKeys } from '@/shared/hooks/workspaceSummaryKeys';
import { controlCenterKeys } from './controlCenterApi';

/** Concurrent create requests; worktree creation on one repo contends on git locks. */
const LAUNCH_CONCURRENCY = 3;

export type LaunchStatus = 'pending' | 'launching' | 'launched' | 'failed';

export interface LaunchTask {
  id: number;
  prompt: string;
  title: string;
  status: LaunchStatus;
  workspaceId?: string;
  error?: string;
}

export interface LaunchTarget {
  repoId: string;
  targetBranch: string;
  executorConfig: ExecutorConfig;
}

async function runWithConcurrency<T>(
  items: T[],
  limit: number,
  worker: (item: T) => Promise<void>
) {
  let next = 0;
  const runners = Array.from({ length: Math.min(limit, items.length) }, () =>
    (async () => {
      while (next < items.length) {
        const item = items[next++];
        await worker(item);
      }
    })()
  );
  await Promise.all(runners);
}

export function useQuickLaunch() {
  const queryClient = useQueryClient();
  const [tasks, setTasks] = useState<LaunchTask[]>([]);
  const [isLaunching, setIsLaunching] = useState(false);
  const nextId = useRef(1);
  const lastTarget = useRef<LaunchTarget | null>(null);

  const updateTask = useCallback((id: number, patch: Partial<LaunchTask>) => {
    setTasks((prev) => prev.map((t) => (t.id === id ? { ...t, ...patch } : t)));
  }, []);

  const launchTasks = useCallback(
    async (toLaunch: LaunchTask[], target: LaunchTarget) => {
      setIsLaunching(true);
      try {
        await runWithConcurrency(toLaunch, LAUNCH_CONCURRENCY, async (task) => {
          updateTask(task.id, { status: 'launching', error: undefined });
          try {
            const { workspace } = await workspacesApi.createAndStart({
              name: task.title,
              prompt: task.prompt,
              repos: [
                { repo_id: target.repoId, target_branch: target.targetBranch },
              ],
              linked_issue: null,
              executor_config: target.executorConfig,
              attachment_ids: null,
            });
            updateTask(task.id, {
              status: 'launched',
              workspaceId: workspace.id,
            });
          } catch (err) {
            updateTask(task.id, {
              status: 'failed',
              error: err instanceof Error ? err.message : String(err),
            });
          }
        });
      } finally {
        setIsLaunching(false);
        void queryClient.invalidateQueries({ queryKey: controlCenterKeys.all });
        void queryClient.invalidateQueries({
          queryKey: workspaceSummaryKeys.all,
        });
      }
    },
    [queryClient, updateTask]
  );

  const launch = useCallback(
    async (prompts: string[], target: LaunchTarget) => {
      const created: LaunchTask[] = prompts.map((prompt) => ({
        id: nextId.current++,
        prompt,
        title: splitMessageToTitleDescription(prompt).title,
        status: 'pending',
      }));
      lastTarget.current = target;
      setTasks(created);
      await launchTasks(created, target);
    },
    [launchTasks]
  );

  const retryFailed = useCallback(async () => {
    const failed = tasks.filter((t) => t.status === 'failed');
    if (failed.length === 0 || !lastTarget.current) return;
    await launchTasks(failed, lastTarget.current);
  }, [launchTasks, tasks]);

  const clear = useCallback(() => setTasks([]), []);

  return { tasks, isLaunching, launch, retryFailed, clear };
}
