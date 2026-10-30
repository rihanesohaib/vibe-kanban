import type {
  AvailabilityInfo,
  BaseCodingAgent,
  ExecutorConfig,
  ExecutorConfigs,
  ExecutorProfile,
  ExecutorAction,
  ExecutorProfileId,
  ExecutionProcess,
} from 'shared/types';

const RESERVED_KEYS = new Set(['recently_used_models']);

/** Executors listed first in every picker, in this order. */
const PREFERRED_EXECUTORS: readonly string[] = ['CURSOR_AGENT'];

/** Sorts executors with preferred ones first, then alphabetically. */
export function sortExecutors<T extends string>(executors: Iterable<T>): T[] {
  return [...executors].sort((a, b) => {
    const rankA = PREFERRED_EXECUTORS.indexOf(a);
    const rankB = PREFERRED_EXECUTORS.indexOf(b);
    if (rankA !== -1 || rankB !== -1) {
      if (rankA === -1) return 1;
      if (rankB === -1) return -1;
      return rankA - rankB;
    }
    return a.localeCompare(b);
  });
}

/**
 * Keeps executors whose CLI is installed, plus any in `keep` (e.g. the
 * current selection). Returns the input unchanged while availability is
 * unknown or when nothing would remain.
 */
export function filterInstalledExecutors<T extends string>(
  executors: T[],
  availability: Record<string, AvailabilityInfo> | null | undefined,
  keep: ReadonlyArray<string | null | undefined> = []
): T[] {
  if (!availability) return executors;
  const filtered = executors.filter(
    (executor) =>
      keep.includes(executor) ||
      (availability[executor] !== undefined &&
        availability[executor].type !== 'NOT_FOUND')
  );
  return filtered.length > 0 ? filtered : executors;
}

export function getExecutorVariantKeys(
  executorProfile: ExecutorProfile | Record<string, unknown> | null | undefined
): string[] {
  return Object.keys(executorProfile || {}).filter(
    (key) => !RESERVED_KEYS.has(key)
  );
}

function sortVariantKeys(variants: string[]): string[] {
  return variants.sort((a, b) => {
    if (a === 'DEFAULT') return -1;
    if (b === 'DEFAULT') return 1;
    return a.localeCompare(b);
  });
}

export function getSortedExecutorVariantKeys(
  executorProfile: ExecutorProfile | Record<string, unknown> | null | undefined
): string[] {
  return sortVariantKeys(getExecutorVariantKeys(executorProfile));
}

/**
 * Compare two ExecutorProfileIds for equality.
 * Treats null/undefined variant as equivalent to "DEFAULT".
 */
export function areProfilesEqual(
  a: ExecutorProfileId | null | undefined,
  b: ExecutorProfileId | null | undefined
): boolean {
  if (!a || !b) return a === b;
  if (a.executor !== b.executor) return false;
  // Normalize variants: null/undefined -> 'DEFAULT'
  const variantA = a.variant ?? 'DEFAULT';
  const variantB = b.variant ?? 'DEFAULT';
  return variantA === variantB;
}

/**
 * Get variant options for a given executor from profiles.
 * Returns variants sorted: DEFAULT first, then alphabetically.
 */
export function getVariantOptions(
  executor: BaseCodingAgent | null | undefined,
  profiles: ExecutorConfigs['executors'] | null | undefined
): string[] {
  if (!executor || !profiles) return [];
  const executorProfile = profiles[executor];
  if (!executorProfile) return [];

  const variants = getExecutorVariantKeys(executorProfile);
  return sortVariantKeys(variants);
}

/**
 * Extract full ExecutorConfig from an ExecutorAction chain.
 * Traverses the action chain to find the first coding agent request.
 */
export function executorConfigFromAction(
  action: ExecutorAction | null
): ExecutorConfig | null {
  let curr: ExecutorAction | null = action;
  while (curr) {
    const typ = curr.typ;
    switch (typ.type) {
      case 'CodingAgentInitialRequest':
      case 'CodingAgentFollowUpRequest':
      case 'ReviewRequest':
        return typ.executor_config;
      case 'ScriptRequest':
      default:
        curr = curr.next_action;
        continue;
    }
  }
  return null;
}

/**
 * Get the full ExecutorConfig from the most recent execution process.
 * Searches from most recent to oldest.
 */
export function getLatestConfigFromProcesses(
  processes: ExecutionProcess[] | undefined
): ExecutorConfig | null {
  if (!processes?.length) return null;
  return (
    processes
      .slice()
      .reverse()
      .map((p) => executorConfigFromAction(p.executor_action ?? null))
      .find((c) => c !== null) ?? null
  );
}
