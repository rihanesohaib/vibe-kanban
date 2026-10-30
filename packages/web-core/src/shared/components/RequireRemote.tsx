import { useEffect, type ReactNode } from 'react';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { isRemoteEnabled } from '@/shared/lib/remoteApi';

/**
 * Renders cloud-only pages when a remote API is configured; otherwise sends
 * the user back to local workspaces.
 */
export function RequireRemote({ children }: { children: ReactNode }) {
  const { loading } = useUserSystem();
  const appNavigation = useAppNavigation();
  const remoteEnabled = isRemoteEnabled();

  useEffect(() => {
    if (!loading && !remoteEnabled) {
      appNavigation.goToWorkspaces({ replace: true });
    }
  }, [appNavigation, loading, remoteEnabled]);

  if (loading || !remoteEnabled) {
    return null;
  }
  return <>{children}</>;
}
