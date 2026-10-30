import { useCallback, useState } from 'react';
import {
  BellIcon,
  CheckCircleIcon,
  CheckIcon,
  ChecksIcon,
  ChatCircleDotsIcon,
  GitMergeIcon,
  HandIcon,
  XCircleIcon,
  type Icon,
} from '@phosphor-icons/react';
import type { AgentAlertKind, AgentInboxItem } from 'shared/types';
import { useAgentInbox } from '@/shared/hooks/useAgentInbox';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import {
  browserNotificationPermission,
  requestBrowserNotificationPermission,
} from '@/shared/lib/agentAlerts';
import { formatRelativeTime } from '@/shared/lib/date';
import { cn } from '@/shared/lib/utils';

const KIND_META: Record<
  AgentAlertKind,
  { label: string; icon: Icon; className: string }
> = {
  finished: {
    label: 'Finished',
    icon: CheckCircleIcon,
    className: 'text-success',
  },
  failed: { label: 'Failed', icon: XCircleIcon, className: 'text-error' },
  needs_approval: {
    label: 'Needs approval',
    icon: HandIcon,
    className: 'text-warning',
  },
  needs_input: {
    label: 'Needs input',
    icon: ChatCircleDotsIcon,
    className: 'text-warning',
  },
  merged: { label: 'Merged', icon: GitMergeIcon, className: 'text-brand' },
};

function BrowserNotificationsBanner() {
  const { config } = useUserSystem();
  const [permission, setPermission] = useState(browserNotificationPermission);

  if (!config?.notifications.push_enabled || permission === 'granted') {
    return null;
  }
  if (permission === null) return null;

  return (
    <div className="flex items-center justify-between gap-base px-double py-half border-b border-border bg-secondary text-sm text-normal">
      {permission === 'denied' ? (
        <span>
          Browser notifications are blocked for this site. Allow them in your
          browser&apos;s site settings to get agent alerts.
        </span>
      ) : (
        <>
          <span>Get a desktop notification when an agent needs you.</span>
          <button
            type="button"
            onClick={() =>
              void requestBrowserNotificationPermission().then(setPermission)
            }
            className="shrink-0 rounded-sm px-base py-half bg-brand text-on-brand hover:bg-brand-hover transition-colors cursor-pointer"
          >
            Enable notifications
          </button>
        </>
      )}
    </div>
  );
}

function InboxRow({
  item,
  onOpen,
  onMarkRead,
}: {
  item: AgentInboxItem;
  onOpen: (item: AgentInboxItem) => void;
  onMarkRead: (item: AgentInboxItem) => void;
}) {
  const meta = KIND_META[item.kind];
  const KindIcon = meta.icon;

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={() => onOpen(item)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onOpen(item);
        }
      }}
      className={cn(
        'w-full flex items-center gap-base px-double py-base text-left transition-colors cursor-pointer outline-none',
        'hover:bg-secondary',
        'focus-visible:bg-secondary',
        'focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-brand',
        !item.read && 'bg-brand/5'
      )}
    >
      <span
        className={cn(
          'shrink-0 w-2 h-2 rounded-full',
          !item.read && 'bg-brand'
        )}
      />
      <KindIcon
        size={18}
        weight="fill"
        className={cn('shrink-0', meta.className)}
        aria-hidden
      />
      <div className="flex-1 min-w-0">
        <p
          className={cn(
            'text-base truncate',
            item.read ? 'text-normal' : 'text-high'
          )}
        >
          <span className="font-medium">{meta.label}</span>
          <span className="text-low"> · </span>
          {item.workspace_name}
        </p>
        <p className="text-sm text-low mt-0.5 truncate">
          {item.detail ? `${item.detail} · ` : ''}
          {formatRelativeTime(item.created_at)}
        </p>
      </div>
      {!item.read && (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onMarkRead(item);
          }}
          onKeyDown={(e) => e.stopPropagation()}
          className={cn(
            'shrink-0 inline-flex items-center gap-half rounded-sm px-half py-half text-sm text-low transition-colors cursor-pointer',
            'hover:bg-secondary hover:text-normal',
            'focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-brand'
          )}
          aria-label="Mark as read"
          title="Mark as read"
        >
          <CheckIcon size={14} weight="bold" />
          <span className="hidden sm:inline">Mark as read</span>
        </button>
      )}
    </div>
  );
}

export function NotificationsPage() {
  const appNavigation = useAppNavigation();
  const { items, unreadCount, isLoading, error, markRead, markAllRead } =
    useAgentInbox();

  const handleOpen = useCallback(
    (item: AgentInboxItem) => {
      if (!item.read) markRead([item.id]);
      appNavigation.goToWorkspace(item.workspace_id);
    },
    [appNavigation, markRead]
  );

  const handleMarkRead = useCallback(
    (item: AgentInboxItem) => markRead([item.id]),
    [markRead]
  );

  return (
    <div className="flex flex-col h-full overflow-hidden">
      <div className="flex items-center justify-between px-double py-base border-b border-border">
        <h1 className="text-xl font-medium text-high">Agent inbox</h1>
        {unreadCount > 0 && (
          <button
            type="button"
            onClick={markAllRead}
            className="flex items-center gap-1 px-base py-half text-sm text-low hover:text-normal transition-colors cursor-pointer"
          >
            <ChecksIcon size={16} />
            Mark all as read
          </button>
        )}
      </div>

      <BrowserNotificationsBanner />

      <div className="flex-1 overflow-y-auto">
        {items.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full gap-2 text-low">
            <BellIcon size={32} weight="light" />
            <p className="text-base">
              {error
                ? 'Could not load agent events'
                : isLoading
                  ? 'Loading…'
                  : 'No agent events in the last 7 days'}
            </p>
          </div>
        ) : (
          <div className="divide-y divide-border">
            {items.map((item) => (
              <InboxRow
                key={item.id}
                item={item}
                onOpen={handleOpen}
                onMarkRead={handleMarkRead}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
