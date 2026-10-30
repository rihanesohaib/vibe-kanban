import { useNavigate } from '@tanstack/react-router';
import { BellIcon } from '@phosphor-icons/react';
import { cn } from '@vibe/ui/lib/cn';
import { Tooltip } from '@vibe/ui/components/Tooltip';
import { useAgentInbox } from '@/shared/hooks/useAgentInbox';

export function AppBarNotificationBellContainer() {
  const navigate = useNavigate();
  const { unreadCount } = useAgentInbox();

  return (
    <Tooltip content="Agent inbox" side="right">
      <button
        type="button"
        onClick={() => navigate({ to: '/notifications' })}
        className={cn(
          'relative flex items-center justify-center w-10 h-10 rounded-lg',
          'text-sm font-medium transition-colors cursor-pointer',
          'focus:outline-none focus-visible:ring-2 focus-visible:ring-brand',
          'bg-panel text-normal hover:opacity-80'
        )}
        aria-label={
          unreadCount > 0 ? `Agent inbox, ${unreadCount} unread` : 'Agent inbox'
        }
      >
        <BellIcon className="w-5 h-5" weight="bold" />
        {unreadCount > 0 && (
          <span className="absolute -top-2 -right-1 min-w-[18px] h-[18px] px-1 flex items-center justify-center rounded-full bg-brand-secondary text-[10px] font-medium text-white">
            {unreadCount > 99 ? '99+' : unreadCount}
          </span>
        )}
      </button>
    </Tooltip>
  );
}
