import type { ControlCenterOverview, HostStats } from 'shared/types';
import { handleApiResponse } from '@/shared/lib/api';
import { makeLocalApiRequest } from '@/shared/lib/localApiTransport';

async function getJson<T>(path: string): Promise<T> {
  const response = await makeLocalApiRequest(path, {
    headers: { 'Content-Type': 'application/json' },
  });
  return handleApiResponse<T>(response);
}

export const controlCenterApi = {
  getHostStats: () => getJson<HostStats>('/api/control-center/host'),
  getOverview: () =>
    getJson<ControlCenterOverview>('/api/control-center/overview'),
};

export const controlCenterKeys = {
  all: ['control-center'] as const,
  host: ['control-center', 'host'] as const,
  overview: ['control-center', 'overview'] as const,
};
