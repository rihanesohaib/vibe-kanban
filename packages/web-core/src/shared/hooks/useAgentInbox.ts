import { useCallback } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { AgentInboxResponse } from 'shared/types';
import { agentInboxApi } from '@/shared/lib/api';

export const AGENT_INBOX_QUERY_KEY = ['agent-inbox'] as const;

const EMPTY_INBOX: AgentInboxResponse = { items: [], unread_count: 0 };

export function useAgentInbox() {
  const queryClient = useQueryClient();

  const query = useQuery({
    queryKey: AGENT_INBOX_QUERY_KEY,
    queryFn: agentInboxApi.list,
    staleTime: 5_000,
    // Live alerts invalidate this immediately; polling catches read state
    // changed elsewhere (e.g. opening a workspace marks its turns seen).
    refetchInterval: 30_000,
  });

  const mutation = useMutation({
    mutationFn: agentInboxApi.markRead,
    onSuccess: (inbox) => {
      queryClient.setQueryData(AGENT_INBOX_QUERY_KEY, inbox);
    },
  });
  const { mutate } = mutation;

  const markRead = useCallback(
    (ids: string[]) => {
      if (ids.length === 0) return;
      mutate({ ids, all: false });
    },
    [mutate]
  );

  const markAllRead = useCallback(() => {
    mutate({ ids: [], all: true });
  }, [mutate]);

  const inbox = query.data ?? EMPTY_INBOX;

  return {
    items: inbox.items,
    unreadCount: inbox.unread_count,
    isLoading: query.isLoading,
    error: query.error,
    markRead,
    markAllRead,
  };
}
