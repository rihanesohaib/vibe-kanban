import { useEffect, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { controlCenterApi, controlCenterKeys } from './controlCenterApi';

const HOST_POLL_MS = 5_000;
const OVERVIEW_POLL_MS = 5_000;

export function useHostStats() {
  return useQuery({
    queryKey: controlCenterKeys.host,
    queryFn: controlCenterApi.getHostStats,
    refetchInterval: HOST_POLL_MS,
    refetchIntervalInBackground: false,
  });
}

export function useControlCenterOverview() {
  return useQuery({
    queryKey: controlCenterKeys.overview,
    queryFn: controlCenterApi.getOverview,
    refetchInterval: OVERVIEW_POLL_MS,
    refetchIntervalInBackground: false,
  });
}

/** Current time in ms, re-rendering every `intervalMs`. */
export function useNow(intervalMs = 1_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs]);
  return now;
}
