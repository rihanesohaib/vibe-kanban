import { useAgentAlerts } from '@/shared/hooks/useAgentAlerts';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { isTauriApp } from '@/shared/lib/platform';
import type { AgentAlert } from 'shared/types';
import { showSystemNotification } from '@web/app/notifications/showSystemNotification';

function showNativeNotification(
  alert: AgentAlert,
  deeplinkPath: string | null
) {
  void showSystemNotification({
    id: alert.id,
    title: alert.title,
    body: alert.body,
    deeplinkPath: deeplinkPath ?? undefined,
  });
}

export function AgentAlerts() {
  const appNavigation = useAppNavigation();

  useAgentAlerts({
    onOpenWorkspace: (workspaceId) => appNavigation.goToWorkspace(workspaceId),
    showNotification: isTauriApp() ? showNativeNotification : undefined,
  });

  return null;
}
