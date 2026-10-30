import { Bot, ArrowDown } from 'lucide-react';
import { Button } from '@vibe/ui/components/Button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@vibe/ui/components/DropdownMenu';
import { Label } from '@vibe/ui/components/Label';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import type { ExecutorProfileId, BaseCodingAgent } from 'shared/types';
import { useVisibleExecutors } from '@/shared/hooks/useVisibleExecutors';

interface AgentSelectorProps {
  profiles: Record<string, Record<string, unknown>> | null;
  selectedExecutorProfile: ExecutorProfileId | null;
  onChange: (profile: ExecutorProfileId) => void;
  disabled?: boolean;
  className?: string;
  showLabel?: boolean;
}

export function AgentSelector({
  profiles,
  selectedExecutorProfile,
  onChange,
  disabled,
  className = '',
  showLabel = false,
}: AgentSelectorProps) {
  const { t } = useTranslation('settings');
  const allAgents = useMemo(
    () => (profiles ? (Object.keys(profiles) as BaseCodingAgent[]) : []),
    [profiles]
  );
  const selectedAgent = selectedExecutorProfile?.executor;
  const {
    executors: agents,
    hiddenCount,
    showAll,
    setShowAll,
  } = useVisibleExecutors(allAgents, [selectedAgent]);

  if (!profiles) return null;

  return (
    <div className="flex-1">
      {showLabel && (
        <Label htmlFor="executor-profile" className="text-sm font-medium">
          Agent
        </Label>
      )}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="outline"
            size="sm"
            className={`w-full justify-between text-xs ${showLabel ? 'mt-1.5' : ''} ${className}`}
            disabled={disabled}
            aria-label="Select agent"
          >
            <div className="flex items-center gap-1.5 w-full">
              <Bot className="h-3 w-3" />
              <span className="truncate">{selectedAgent || 'Agent'}</span>
            </div>
            <ArrowDown className="h-3 w-3" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent className="w-60">
          {agents.length === 0 ? (
            <div className="p-2 text-sm text-muted-foreground text-center">
              No agents available
            </div>
          ) : (
            agents.map((agent) => (
              <DropdownMenuItem
                key={agent}
                onClick={() => {
                  onChange({
                    executor: agent,
                    variant: null,
                  });
                }}
                className={selectedAgent === agent ? 'bg-accent' : ''}
              >
                {agent}
              </DropdownMenuItem>
            ))
          )}
          {hiddenCount > 0 && (
            <DropdownMenuItem
              onSelect={(event) => {
                event.preventDefault();
                setShowAll(!showAll);
              }}
              className="text-muted-foreground"
            >
              {showAll
                ? t('settings.agents.hideUninstalled')
                : t('settings.agents.showUninstalled', { count: hiddenCount })}
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
