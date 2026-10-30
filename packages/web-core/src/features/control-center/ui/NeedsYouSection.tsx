import { ArrowRightIcon } from '@phosphor-icons/react';
import type { ControlCenterWorkspace } from 'shared/types';
import { cn } from '@/shared/lib/utils';
import {
  STATE_META,
  elapsedSeconds,
  formatDuration,
  repoLabel,
  stateSince,
  workspaceTitle,
} from '../model/format';
import { Panel, PanelMessage } from './Panel';
import { WorkspaceGlance } from './WorkspaceGlance';

function NeedsYouCard({
  workspace,
  nowMs,
  onOpen,
}: {
  workspace: ControlCenterWorkspace;
  nowMs: number;
  onOpen: (workspaceId: string) => void;
}) {
  const meta = STATE_META[workspace.state];
  return (
    <WorkspaceGlance workspace={workspace} nowMs={nowMs}>
      <button
        type="button"
        onClick={() => onOpen(workspace.workspace_id)}
        className={cn(
          'group flex min-w-0 flex-col gap-half rounded-sm border border-border bg-primary p-base text-left',
          'cursor-pointer transition-colors hover:border-brand/60',
          'focus:outline-none focus-visible:ring-1 focus-visible:ring-brand'
        )}
      >
        <span className="flex items-center gap-half text-sm">
          <span
            className={cn('size-2 rounded-full', meta.dotClassName)}
            aria-hidden="true"
          />
          <span className={meta.textClassName}>{meta.label}</span>
          <span className="ml-auto tabular-nums text-low">
            {formatDuration(elapsedSeconds(stateSince(workspace), nowMs))}
          </span>
        </span>
        <span className="truncate text-lg font-medium text-high">
          {workspaceTitle(workspace)}
        </span>
        <span className="flex items-center gap-half text-sm text-low">
          <span className="truncate">{repoLabel(workspace)}</span>
          <span aria-hidden="true">·</span>
          <span className="truncate font-ibm-plex-mono">
            {workspace.branch}
          </span>
          <ArrowRightIcon
            className="ml-auto size-icon-sm shrink-0 text-low group-hover:text-brand"
            weight="bold"
          />
        </span>
      </button>
    </WorkspaceGlance>
  );
}

export function NeedsYouSection({
  workspaces,
  isLoading,
  nowMs,
  onOpen,
}: {
  workspaces: ControlCenterWorkspace[];
  isLoading: boolean;
  nowMs: number;
  onOpen: (workspaceId: string) => void;
}) {
  return (
    <Panel
      title={
        <span className="flex items-center gap-base">
          Needs you
          {workspaces.length > 0 && (
            <span className="rounded-sm bg-primary px-half text-sm tabular-nums text-normal">
              {workspaces.length}
            </span>
          )}
        </span>
      }
    >
      {isLoading ? (
        <PanelMessage>Loading…</PanelMessage>
      ) : workspaces.length === 0 ? (
        <PanelMessage>
          Nothing is waiting on you. Agents that need approval or have failed
          will show up here.
        </PanelMessage>
      ) : (
        <div className="grid grid-cols-1 gap-base xl:grid-cols-2">
          {workspaces.map((ws) => (
            <NeedsYouCard
              key={ws.workspace_id}
              workspace={ws}
              nowMs={nowMs}
              onOpen={onOpen}
            />
          ))}
        </div>
      )}
    </Panel>
  );
}
