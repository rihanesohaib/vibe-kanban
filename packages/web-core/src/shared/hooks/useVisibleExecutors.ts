import { useCallback, useMemo, useSyncExternalStore } from 'react';
import { useQuery } from '@tanstack/react-query';
import type { AvailabilityInfo } from 'shared/types';
import { configApi } from '@/shared/lib/api';
import { filterInstalledExecutors, sortExecutors } from '@/shared/lib/executor';

const SHOW_ALL_STORAGE_KEY = 'vk.showAllExecutors';
const SHOW_ALL_EVENT = 'vk-show-all-executors-change';

function readShowAll(): boolean {
  try {
    return localStorage.getItem(SHOW_ALL_STORAGE_KEY) === 'true';
  } catch {
    // localStorage may be unavailable
    return false;
  }
}

function subscribeShowAll(onChange: () => void): () => void {
  window.addEventListener(SHOW_ALL_EVENT, onChange);
  window.addEventListener('storage', onChange);
  return () => {
    window.removeEventListener(SHOW_ALL_EVENT, onChange);
    window.removeEventListener('storage', onChange);
  };
}

export function useAgentAvailability() {
  return useQuery<Record<string, AvailabilityInfo>>({
    queryKey: ['agent-availability'],
    queryFn: () => configApi.getAllAgentAvailability(),
    staleTime: 60 * 1000,
    retry: 1,
  });
}

/** Persisted toggle for listing executors whose CLI is not installed. */
export function useShowAllExecutors(): [boolean, (value: boolean) => void] {
  const showAll = useSyncExternalStore(
    subscribeShowAll,
    readShowAll,
    () => false
  );
  const setShowAll = useCallback((value: boolean) => {
    try {
      localStorage.setItem(SHOW_ALL_STORAGE_KEY, String(value));
    } catch {
      // localStorage may be unavailable
    }
    window.dispatchEvent(new Event(SHOW_ALL_EVENT));
  }, []);
  return [showAll, setShowAll];
}

/**
 * Executors to offer in a picker: preferred (Cursor) first, uninstalled ones
 * hidden unless the user opted to show all. Entries in `keep` always stay so
 * existing selections referencing other executors keep working.
 */
export function useVisibleExecutors<T extends string>(
  executors: T[],
  keep: ReadonlyArray<string | null | undefined> = []
) {
  const { data: availability } = useAgentAvailability();
  const [showAll, setShowAll] = useShowAllExecutors();
  const keepKey = keep.join('|');

  return useMemo(() => {
    const sorted = sortExecutors(executors);
    const installed = filterInstalledExecutors(
      sorted,
      availability,
      keepKey.split('|')
    );
    return {
      executors: showAll ? sorted : installed,
      hiddenCount: sorted.length - installed.length,
      showAll,
      setShowAll,
    };
  }, [executors, availability, keepKey, showAll, setShowAll]);
}
