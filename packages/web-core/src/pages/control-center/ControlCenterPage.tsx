import { useCallback, useMemo, useRef, useState } from 'react';
import type { ControlCenterWorkspace } from 'shared/types';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import {
  useControlCenterOverview,
  useHostStats,
  useNow,
} from '@/features/control-center/model/useControlCenter';
import {
  STATE_META,
  isFailedToday,
} from '@/features/control-center/model/format';
import { KpiTile } from '@/features/control-center/ui/KpiTile';
import { NeedsYouSection } from '@/features/control-center/ui/NeedsYouSection';
import {
  AgentsList,
  type AgentsFilter,
} from '@/features/control-center/ui/AgentsList';
import { HostPanel } from '@/features/control-center/ui/HostPanel';
import { QuickLaunchPanel } from '@/features/control-center/ui/QuickLaunchPanel';

const FILTERS: Record<
  AgentsFilter,
  (ws: ControlCenterWorkspace, now: Date) => boolean
> = {
  active: (ws) => ws.state === 'running' || ws.state === 'waiting',
  running: (ws) => ws.state === 'running',
  waiting: (ws) => ws.state === 'waiting',
  review: (ws) => ws.state === 'review',
  failed_today: (ws, now) => isFailedToday(ws, now),
  all: () => true,
};

export function ControlCenterPage() {
  usePageTitle('Control Center');
  const appNavigation = useAppNavigation();
  const overviewQuery = useControlCenterOverview();
  const hostQuery = useHostStats();
  const nowMs = useNow(1_000);
  const [filter, setFilter] = useState<AgentsFilter>('active');
  const agentsRef = useRef<HTMLDivElement>(null);

  const workspaces = useMemo(
    () => overviewQuery.data?.workspaces ?? [],
    [overviewQuery.data]
  );
  const isOverviewLoading = overviewQuery.isLoading;

  // Recompute day boundaries at most once a minute.
  const minuteBucket = Math.floor(nowMs / 60_000);
  const filterCounts = useMemo(() => {
    const now = new Date(minuteBucket * 60_000);
    const entries = (Object.keys(FILTERS) as AgentsFilter[]).map(
      (key) =>
        [key, workspaces.filter((ws) => FILTERS[key](ws, now)).length] as const
    );
    return Object.fromEntries(entries) as Record<AgentsFilter, number>;
  }, [workspaces, minuteBucket]);

  const filtered = useMemo(() => {
    const now = new Date(minuteBucket * 60_000);
    return workspaces.filter((ws) => FILTERS[filter](ws, now));
  }, [workspaces, filter, minuteBucket]);

  const needsYou = useMemo(() => {
    const now = new Date(minuteBucket * 60_000);
    return workspaces
      .filter(
        (ws) =>
          ws.state === 'waiting' ||
          (ws.state === 'failed' && isFailedToday(ws, now))
      )
      .sort((a, b) => {
        if (a.state !== b.state) return a.state === 'waiting' ? -1 : 1;
        return b.last_activity_at.localeCompare(a.last_activity_at);
      });
  }, [workspaces, minuteBucket]);

  const openWorkspace = useCallback(
    (workspaceId: string) => appNavigation.goToWorkspace(workspaceId),
    [appNavigation]
  );

  const selectFilter = useCallback((next: AgentsFilter) => {
    setFilter(next);
    agentsRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }, []);

  const counts = overviewQuery.data?.counts;
  const kpiValue = (value: number | undefined) =>
    isOverviewLoading ? null : (value ?? 0);

  return (
    <div className="h-full overflow-y-auto bg-primary">
      <div className="mx-auto flex max-w-[1600px] flex-col gap-double p-double">
        <header className="flex items-baseline gap-base">
          <h1 className="text-2xl font-semibold text-high">Control Center</h1>
          {overviewQuery.isError && (
            <span className="text-base text-error">
              Could not load workspaces. Retrying…
            </span>
          )}
          {counts && (
            <span className="ml-auto text-base tabular-nums text-low">
              {counts.total} workspace{counts.total === 1 ? '' : 's'}
            </span>
          )}
        </header>

        <div className="grid grid-cols-2 gap-double xl:grid-cols-4">
          <KpiTile
            label="Running agents"
            value={kpiValue(counts?.running)}
            hint="Agents working right now"
            dotClassName={STATE_META.running.dotClassName}
            isActive={filter === 'running'}
            onClick={() => selectFilter('running')}
          />
          <KpiTile
            label="Waiting on you"
            value={kpiValue(counts?.waiting)}
            hint="Blocked on an approval or question"
            dotClassName={STATE_META.waiting.dotClassName}
            isActive={filter === 'waiting'}
            onClick={() => selectFilter('waiting')}
          />
          <KpiTile
            label="Ready for review"
            value={kpiValue(counts?.review)}
            hint="Finished with unseen results"
            dotClassName={STATE_META.review.dotClassName}
            isActive={filter === 'review'}
            onClick={() => selectFilter('review')}
          />
          <KpiTile
            label="Failed today"
            value={isOverviewLoading ? null : filterCounts.failed_today}
            hint="Latest run failed or was stopped"
            dotClassName={STATE_META.failed.dotClassName}
            isActive={filter === 'failed_today'}
            onClick={() => selectFilter('failed_today')}
          />
        </div>

        <div className="grid grid-cols-1 gap-double xl:grid-cols-3">
          <div className="flex min-w-0 flex-col gap-double xl:col-span-2">
            <NeedsYouSection
              workspaces={needsYou}
              isLoading={isOverviewLoading}
              nowMs={nowMs}
              onOpen={openWorkspace}
            />
            <div ref={agentsRef} className="scroll-mt-double">
              <AgentsList
                workspaces={filtered}
                filter={filter}
                counts={filterCounts}
                isLoading={isOverviewLoading}
                nowMs={nowMs}
                onFilterChange={setFilter}
                onOpen={openWorkspace}
              />
            </div>
          </div>
          <div className="flex min-w-0 flex-col gap-double">
            <HostPanel
              stats={hostQuery.data}
              isLoading={hostQuery.isLoading}
              isError={hostQuery.isError}
            />
            <QuickLaunchPanel onOpenWorkspace={openWorkspace} />
          </div>
        </div>
      </div>
    </div>
  );
}
