import type { ReactNode } from 'react';
import * as TooltipPrimitive from '@radix-ui/react-tooltip';
import type { ControlCenterWorkspace } from 'shared/types';
import { cn } from '@/shared/lib/utils';
import {
  STATE_META,
  executorLabel,
  formatDuration,
  elapsedSeconds,
  repoLabel,
  stateSince,
  workspaceTitle,
} from '../model/format';

function GlanceRow({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex gap-base">
      <dt className="w-20 shrink-0 text-low">{label}</dt>
      <dd className="min-w-0 truncate text-normal">{value}</dd>
    </div>
  );
}

export function WorkspaceGlance({
  workspace,
  nowMs,
  children,
}: {
  workspace: ControlCenterWorkspace;
  nowMs: number;
  children: ReactNode;
}) {
  const meta = STATE_META[workspace.state];
  const diff = workspace.diff_stats;

  return (
    <TooltipPrimitive.Provider delayDuration={350}>
      <TooltipPrimitive.Root>
        <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
        <TooltipPrimitive.Portal>
          <TooltipPrimitive.Content
            side="right"
            align="start"
            sideOffset={8}
            collisionPadding={12}
            className={cn(
              'z-[10000] w-80 rounded-sm border border-border bg-panel p-base shadow-md',
              'animate-in fade-in-0 zoom-in-95'
            )}
          >
            <div className="flex items-center gap-half">
              <span
                className={cn('size-2 rounded-full', meta.dotClassName)}
                aria-hidden="true"
              />
              <span className={cn('text-sm font-medium', meta.textClassName)}>
                {meta.label}
              </span>
              <span className="ml-auto text-sm tabular-nums text-low">
                {formatDuration(elapsedSeconds(stateSince(workspace), nowMs))}
              </span>
            </div>
            <p className="mt-half text-lg font-medium text-high line-clamp-2">
              {workspaceTitle(workspace)}
            </p>
            <dl className="mt-base flex flex-col gap-half text-sm">
              <GlanceRow label="Repo" value={repoLabel(workspace)} />
              <GlanceRow
                label="Branch"
                value={
                  <span className="font-ibm-plex-mono">{workspace.branch}</span>
                }
              />
              {workspace.repos[0] && (
                <GlanceRow
                  label="Base"
                  value={
                    <span className="font-ibm-plex-mono">
                      {workspace.repos[0].target_branch}
                    </span>
                  }
                />
              )}
              <GlanceRow
                label="Agent"
                value={executorLabel(workspace) ?? 'Not started'}
              />
              {diff && (
                <GlanceRow
                  label="Changes"
                  value={
                    <span className="tabular-nums">
                      {diff.files_changed} files{' '}
                      <span className="text-success">+{diff.lines_added}</span>{' '}
                      <span className="text-error">-{diff.lines_removed}</span>
                    </span>
                  }
                />
              )}
              {workspace.has_running_dev_server && (
                <GlanceRow label="Dev server" value="Running" />
              )}
            </dl>
            <p className="mt-base text-xs text-low">Click to open workspace</p>
          </TooltipPrimitive.Content>
        </TooltipPrimitive.Portal>
      </TooltipPrimitive.Root>
    </TooltipPrimitive.Provider>
  );
}
