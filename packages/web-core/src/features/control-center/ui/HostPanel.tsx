import type { ReactNode } from 'react';
import type { ByteUsage, DiskUsage, HostStats } from 'shared/types';
import { cn } from '@/shared/lib/utils';
import {
  GAUGE_FILL_CLASS,
  GAUGE_TEXT_CLASS,
  formatBytes,
  formatDuration,
  gaugeLevel,
} from '../model/format';
import { Panel, PanelMessage } from './Panel';

interface Thresholds {
  warn: number;
  critical: number;
}

const CPU_THRESHOLDS: Thresholds = { warn: 75, critical: 90 };
const MEMORY_THRESHOLDS: Thresholds = { warn: 80, critical: 92 };
const SWAP_THRESHOLDS: Thresholds = { warn: 25, critical: 60 };
const DISK_THRESHOLDS: Thresholds = { warn: 80, critical: 90 };

function Gauge({
  label,
  percent,
  detail,
  thresholds,
  title,
}: {
  label: string;
  percent: number | null;
  detail: ReactNode;
  thresholds: Thresholds;
  title?: string;
}) {
  const clamped = percent === null ? null : Math.min(100, Math.max(0, percent));
  const level =
    clamped === null
      ? 'ok'
      : gaugeLevel(clamped, thresholds.warn, thresholds.critical);

  return (
    <div className="flex flex-col gap-half" title={title}>
      <div className="flex items-baseline gap-base">
        <span className="text-base text-normal">{label}</span>
        <span
          className={cn(
            'ml-auto text-xl font-semibold tabular-nums',
            clamped === null ? 'text-low' : GAUGE_TEXT_CLASS[level]
          )}
        >
          {clamped === null ? '–' : `${Math.round(clamped)}%`}
        </span>
      </div>
      <div
        className="h-2 w-full overflow-hidden rounded-sm bg-primary"
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={clamped ?? undefined}
      >
        {clamped !== null && (
          <div
            className={cn('h-full rounded-sm', GAUGE_FILL_CLASS[level])}
            style={{ width: `${clamped}%` }}
          />
        )}
      </div>
      <span className="text-sm tabular-nums text-low">{detail}</span>
    </div>
  );
}

function usagePercent(usage: ByteUsage | DiskUsage | null): number | null {
  if (!usage || usage.total_bytes <= 0) return null;
  return (usage.used_bytes / usage.total_bytes) * 100;
}

function usageDetail(usage: ByteUsage | DiskUsage | null): string {
  if (!usage) return 'Not available on this OS';
  return `${formatBytes(usage.used_bytes)} of ${formatBytes(usage.total_bytes)}`;
}

export function HostPanel({
  stats,
  isLoading,
  isError,
}: {
  stats: HostStats | undefined;
  isLoading: boolean;
  isError: boolean;
}) {
  if (!stats) {
    return (
      <Panel title="Host">
        <PanelMessage>
          {isLoading ? 'Loading…' : isError ? 'Host stats unavailable.' : '–'}
        </PanelMessage>
      </Panel>
    );
  }

  const load = stats.load_average;
  const loadPerCpu =
    load && stats.cpu_count > 0 ? (load.one / stats.cpu_count) * 100 : null;
  const sameDisk =
    stats.workspace_disk &&
    stats.root_disk &&
    stats.workspace_disk.total_bytes === stats.root_disk.total_bytes &&
    stats.workspace_disk.used_bytes === stats.root_disk.used_bytes;

  return (
    <Panel
      title="Host"
      actions={
        <span className="text-sm tabular-nums text-low">
          {stats.cpu_count} vCPU
          {stats.uptime_seconds !== null &&
            ` · up ${formatDuration(stats.uptime_seconds)}`}
        </span>
      }
    >
      <div className="flex flex-col gap-double">
        <Gauge
          label="CPU"
          percent={stats.cpu_utilization_percent}
          thresholds={CPU_THRESHOLDS}
          detail={
            load
              ? `Load ${load.one.toFixed(2)} · ${load.five.toFixed(2)} · ${load.fifteen.toFixed(2)}`
              : 'Load average not available'
          }
        />
        <Gauge
          label="Load / vCPU"
          percent={loadPerCpu}
          thresholds={CPU_THRESHOLDS}
          detail="1-minute load relative to vCPU count"
        />
        <Gauge
          label="Memory"
          percent={usagePercent(stats.memory)}
          thresholds={MEMORY_THRESHOLDS}
          detail={usageDetail(stats.memory)}
        />
        {stats.swap && stats.swap.total_bytes > 0 && (
          <Gauge
            label="Swap"
            percent={usagePercent(stats.swap)}
            thresholds={SWAP_THRESHOLDS}
            detail={usageDetail(stats.swap)}
          />
        )}
        <Gauge
          label={sameDisk ? 'Disk (workspaces, /)' : 'Disk (workspaces)'}
          percent={usagePercent(stats.workspace_disk)}
          thresholds={DISK_THRESHOLDS}
          detail={usageDetail(stats.workspace_disk)}
          title={stats.workspace_disk?.path}
        />
        {!sameDisk && (
          <Gauge
            label="Disk (/)"
            percent={usagePercent(stats.root_disk)}
            thresholds={DISK_THRESHOLDS}
            detail={usageDetail(stats.root_disk)}
            title={stats.root_disk?.path}
          />
        )}
      </div>
    </Panel>
  );
}
