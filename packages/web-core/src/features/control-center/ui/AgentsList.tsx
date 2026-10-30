import type { ControlCenterWorkspace } from 'shared/types';
import { cn } from '@/shared/lib/utils';
import {
  STATE_META,
  elapsedSeconds,
  executorLabel,
  formatDuration,
  repoLabel,
  stateSince,
  workspaceTitle,
} from '../model/format';
import { Panel, PanelMessage } from './Panel';
import { WorkspaceGlance } from './WorkspaceGlance';

export type AgentsFilter =
  'active' | 'running' | 'waiting' | 'review' | 'failed_today' | 'all';

export const AGENTS_FILTER_LABELS: Record<AgentsFilter, string> = {
  active: 'Active',
  running: 'Running',
  waiting: 'Waiting on you',
  review: 'Ready for review',
  failed_today: 'Failed today',
  all: 'All',
};

const FILTER_ORDER: AgentsFilter[] = [
  'active',
  'running',
  'waiting',
  'review',
  'failed_today',
  'all',
];

const EMPTY_MESSAGES: Record<AgentsFilter, string> = {
  active: 'No agents are running right now. Use Quick launch to start some.',
  running: 'No agents are running.',
  waiting: 'No agents are waiting on you.',
  review: 'Nothing is ready for review.',
  failed_today: 'No failures today.',
  all: 'No workspaces yet.',
};

function AgentRow({
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
      <tr
        tabIndex={0}
        onClick={() => onOpen(workspace.workspace_id)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            onOpen(workspace.workspace_id);
          }
        }}
        className="cursor-pointer border-b border-border last:border-b-0 hover:bg-primary focus:outline-none focus-visible:bg-primary"
      >
        <td className="py-base pl-half pr-base">
          <span
            className={cn('block size-2 rounded-full', meta.dotClassName)}
            aria-label={meta.label}
          />
        </td>
        <td className="max-w-0 py-base pr-base">
          <div className="truncate text-base text-high">
            {workspaceTitle(workspace)}
          </div>
          <div className="truncate text-sm text-low">
            {repoLabel(workspace)}
          </div>
        </td>
        <td className="hidden max-w-0 py-base pr-base lg:table-cell">
          <span className="block truncate font-ibm-plex-mono text-sm text-normal">
            {workspace.branch}
          </span>
        </td>
        <td className="hidden max-w-0 py-base pr-base xl:table-cell">
          <span className="block truncate text-sm text-normal">
            {executorLabel(workspace) ?? '–'}
          </span>
        </td>
        <td className="py-base pr-base text-right text-sm tabular-nums text-normal">
          {formatDuration(elapsedSeconds(stateSince(workspace), nowMs))}
        </td>
      </tr>
    </WorkspaceGlance>
  );
}

export function AgentsList({
  workspaces,
  filter,
  counts,
  isLoading,
  nowMs,
  onFilterChange,
  onOpen,
}: {
  workspaces: ControlCenterWorkspace[];
  filter: AgentsFilter;
  counts: Record<AgentsFilter, number>;
  isLoading: boolean;
  nowMs: number;
  onFilterChange: (filter: AgentsFilter) => void;
  onOpen: (workspaceId: string) => void;
}) {
  return (
    <Panel title="Agents">
      <div
        className="mb-base flex flex-wrap gap-half"
        role="tablist"
        aria-label="Filter agents"
      >
        {FILTER_ORDER.map((key) => (
          <button
            key={key}
            type="button"
            role="tab"
            aria-selected={filter === key}
            onClick={() => onFilterChange(key)}
            className={cn(
              'rounded-sm border px-base py-half text-sm cursor-pointer transition-colors',
              'focus:outline-none focus-visible:ring-1 focus-visible:ring-brand',
              filter === key
                ? 'border-brand bg-brand/10 text-high'
                : 'border-border text-low hover:text-normal'
            )}
          >
            {AGENTS_FILTER_LABELS[key]}{' '}
            <span className="tabular-nums">{counts[key]}</span>
          </button>
        ))}
      </div>
      {isLoading ? (
        <PanelMessage>Loading…</PanelMessage>
      ) : workspaces.length === 0 ? (
        <PanelMessage>{EMPTY_MESSAGES[filter]}</PanelMessage>
      ) : (
        <table className="w-full table-fixed border-collapse">
          <colgroup>
            <col className="w-6" />
            <col />
            <col className="hidden w-[28%] lg:table-column" />
            <col className="hidden w-[22%] xl:table-column" />
            <col className="w-20" />
          </colgroup>
          <thead>
            <tr className="border-b border-border text-left text-sm text-low">
              <th className="pb-half font-normal" aria-label="Status" />
              <th className="pb-half pr-base font-normal">Task</th>
              <th className="hidden pb-half pr-base font-normal lg:table-cell">
                Branch
              </th>
              <th className="hidden pb-half pr-base font-normal xl:table-cell">
                Agent
              </th>
              <th className="pb-half pr-base text-right font-normal">
                Elapsed
              </th>
            </tr>
          </thead>
          <tbody>
            {workspaces.map((ws) => (
              <AgentRow
                key={ws.workspace_id}
                workspace={ws}
                nowMs={nowMs}
                onOpen={onOpen}
              />
            ))}
          </tbody>
        </table>
      )}
    </Panel>
  );
}
