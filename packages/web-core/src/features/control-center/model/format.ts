import type {
  ControlCenterWorkspace,
  ControlCenterWorkspaceState,
} from 'shared/types';

export interface StateMeta {
  label: string;
  dotClassName: string;
  textClassName: string;
}

// Status colours come from theme tokens only.
export const STATE_META: Record<ControlCenterWorkspaceState, StateMeta> = {
  running: {
    label: 'Running',
    dotClassName: 'bg-[hsl(var(--info))]',
    textClassName: 'text-[hsl(var(--info))]',
  },
  waiting: {
    label: 'Waiting on you',
    dotClassName: 'bg-[hsl(var(--warning))]',
    textClassName: 'text-[hsl(var(--warning))]',
  },
  review: {
    label: 'Ready for review',
    dotClassName: 'bg-success',
    textClassName: 'text-success',
  },
  failed: {
    label: 'Failed',
    dotClassName: 'bg-error',
    textClassName: 'text-error',
  },
  idle: {
    label: 'Idle',
    dotClassName: 'bg-low',
    textClassName: 'text-low',
  },
};

export type GaugeLevel = 'ok' | 'warn' | 'critical';

export function gaugeLevel(
  percent: number,
  warnAt: number,
  criticalAt: number
): GaugeLevel {
  if (percent >= criticalAt) return 'critical';
  if (percent >= warnAt) return 'warn';
  return 'ok';
}

export const GAUGE_FILL_CLASS: Record<GaugeLevel, string> = {
  ok: 'bg-brand',
  warn: 'bg-[hsl(var(--warning))]',
  critical: 'bg-error',
};

export const GAUGE_TEXT_CLASS: Record<GaugeLevel, string> = {
  ok: 'text-high',
  warn: 'text-[hsl(var(--warning))]',
  critical: 'text-error',
};

const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const exponent = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    BYTE_UNITS.length - 1
  );
  const value = bytes / 1024 ** exponent;
  const digits = value >= 100 || exponent === 0 ? 0 : 1;
  return `${value.toFixed(digits)} ${BYTE_UNITS[exponent]}`;
}

export function formatDuration(totalSeconds: number): string {
  const s = Math.max(0, Math.floor(totalSeconds));
  const days = Math.floor(s / 86400);
  const hours = Math.floor((s % 86400) / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  const seconds = s % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, '0')}m`;
  return `${minutes}:${String(seconds).padStart(2, '0')}`;
}

export function elapsedSeconds(since: string | null, nowMs: number): number {
  if (!since) return 0;
  const start = new Date(since).getTime();
  if (Number.isNaN(start)) return 0;
  return Math.max(0, (nowMs - start) / 1000);
}

/** The timestamp the elapsed timer for a workspace counts from. */
export function stateSince(ws: ControlCenterWorkspace): string {
  if (ws.state === 'running' || ws.state === 'waiting') {
    return ws.latest_process_started_at ?? ws.last_activity_at;
  }
  return ws.latest_process_completed_at ?? ws.last_activity_at;
}

export function isToday(iso: string | null, now = new Date()): boolean {
  if (!iso) return false;
  const date = new Date(iso);
  return (
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate()
  );
}

export function isFailedToday(ws: ControlCenterWorkspace, now = new Date()) {
  return (
    ws.state === 'failed' &&
    isToday(ws.latest_process_completed_at ?? ws.last_activity_at, now)
  );
}

export function workspaceTitle(ws: ControlCenterWorkspace): string {
  return ws.name?.trim() || ws.branch;
}

export function repoLabel(ws: ControlCenterWorkspace): string {
  if (ws.repos.length === 0) return 'No repo';
  return ws.repos.map((r) => r.name).join(', ');
}

export function executorLabel(ws: ControlCenterWorkspace): string | null {
  if (!ws.executor) return null;
  const executor = ws.executor
    .toLowerCase()
    .split('_')
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ');
  const detail =
    ws.model_id ?? (ws.variant && ws.variant !== 'DEFAULT' ? ws.variant : null);
  return detail ? `${executor} · ${detail}` : executor;
}
