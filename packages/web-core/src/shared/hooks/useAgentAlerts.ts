import { useEffect, useRef } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type { AgentAlert } from 'shared/types';
import { AGENT_ALERT_STREAM_URL, agentInboxApi } from '@/shared/lib/api';
import {
  browserNotificationPermission,
  claimAlert,
  hasAskedNotificationPermission,
  playAlertSound,
  requestBrowserNotificationPermission,
  showBrowserNotification,
} from '@/shared/lib/agentAlerts';
import { AGENT_INBOX_QUERY_KEY } from '@/shared/hooks/useAgentInbox';
import { useUserSystem } from '@/shared/hooks/useUserSystem';

interface UseAgentAlertsOptions {
  onOpenWorkspace: (workspaceId: string) => void;
  /**
   * Replaces the browser Notification API (e.g. native notifications in the
   * desktop app). Receives the path to open when the notification is clicked.
   */
  showNotification?: (alert: AgentAlert, deeplinkPath: string | null) => void;
}

/**
 * Listens to the server's live agent alert stream and, per the user's
 * notification settings, plays the alert sound and shows a notification in
 * this browser. Also keeps the agent inbox fresh.
 */
export function useAgentAlerts({
  onOpenWorkspace,
  showNotification,
}: UseAgentAlertsOptions) {
  const queryClient = useQueryClient();
  const { config } = useUserSystem();
  const settings = config?.notifications;

  const latest = useRef({ settings, onOpenWorkspace, showNotification });
  latest.current = { settings, onOpenWorkspace, showNotification };

  useEffect(() => {
    if (typeof EventSource === 'undefined') return;

    const openAlert = (alert: AgentAlert) => {
      void agentInboxApi
        .markRead({ ids: [alert.id], all: false })
        .then((inbox) => queryClient.setQueryData(AGENT_INBOX_QUERY_KEY, inbox))
        .catch(() => undefined);
      if (alert.workspace_id) {
        latest.current.onOpenWorkspace(alert.workspace_id);
      }
    };

    const deliver = async (alert: AgentAlert) => {
      const current = latest.current.settings;
      if (!current || (!current.sound_enabled && !current.push_enabled)) {
        return;
      }
      if (!(await claimAlert(alert.id))) return;

      if (current.sound_enabled) {
        playAlertSound(current.sound_file);
      }
      if (current.push_enabled) {
        const custom = latest.current.showNotification;
        if (custom) {
          custom(
            alert,
            alert.workspace_id ? `/workspaces/${alert.workspace_id}` : null
          );
        } else {
          showBrowserNotification(alert, () => openAlert(alert));
        }
      }
    };

    const source = new EventSource(AGENT_ALERT_STREAM_URL);
    const handleAlert = (event: MessageEvent<string>) => {
      let alert: AgentAlert;
      try {
        alert = JSON.parse(event.data) as AgentAlert;
      } catch {
        return;
      }
      void queryClient.invalidateQueries({ queryKey: AGENT_INBOX_QUERY_KEY });
      void deliver(alert);
    };
    source.addEventListener('alert', handleAlert);

    return () => {
      source.removeEventListener('alert', handleAlert);
      source.close();
    };
  }, [queryClient]);

  // Browsers only show the permission prompt in response to a user gesture,
  // so ask once on the first interaction after push alerts are enabled.
  const pushEnabled = !!settings?.push_enabled;
  const usesBrowserNotifications = !showNotification;
  useEffect(() => {
    if (!pushEnabled || !usesBrowserNotifications) return;
    if (browserNotificationPermission() !== 'default') return;
    if (hasAskedNotificationPermission()) return;

    function cleanup() {
      window.removeEventListener('pointerdown', ask, true);
      window.removeEventListener('keydown', ask, true);
    }
    function ask() {
      cleanup();
      void requestBrowserNotificationPermission();
    }
    window.addEventListener('pointerdown', ask, true);
    window.addEventListener('keydown', ask, true);
    return cleanup;
  }, [pushEnabled, usesBrowserNotifications]);
}
