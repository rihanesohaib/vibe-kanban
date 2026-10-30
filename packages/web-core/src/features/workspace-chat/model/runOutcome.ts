import { ExecutionProcessStatus, type ExecutionProcess } from 'shared/types';

/**
 * How a finished run ended when it did not complete normally.
 * - `stopped`: the user pressed Stop (status `killed`).
 * - `interrupted`: the service shut down or the agent was terminated by a
 *   signal (status `failed` with no exit code, -1, or 128 + signal).
 * - `failed`: the agent exited with a non-zero exit code.
 */
export type RunOutcome =
  | { readonly kind: 'stopped' }
  | { readonly kind: 'interrupted' }
  | { readonly kind: 'failed'; readonly exitCode: number };

function isSignalExit(code: number): boolean {
  return code === -1 || (code > 128 && code < 160);
}

export function getRunOutcome(
  process: Pick<ExecutionProcess, 'status' | 'exit_code'> | null | undefined
): RunOutcome | null {
  if (!process) return null;
  if (process.status === ExecutionProcessStatus.killed) {
    return { kind: 'stopped' };
  }
  if (process.status !== ExecutionProcessStatus.failed) return null;

  const code = process.exit_code === null ? null : Number(process.exit_code);
  if (code === null || Number.isNaN(code) || isSignalExit(code)) {
    return { kind: 'interrupted' };
  }
  return { kind: 'failed', exitCode: code };
}
