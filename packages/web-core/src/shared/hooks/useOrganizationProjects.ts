import { useShape } from '@/shared/integrations/electric/hooks';
import { PROJECTS_SHAPE } from 'shared/remote-types';
import { useAuth } from '@/shared/hooks/auth/useAuth';
import { isLocalKanbanMode } from '@/shared/lib/remoteApi';

export function useOrganizationProjects(organizationId: string | null) {
  const { isSignedIn } = useAuth();

  const enabled = (isSignedIn || isLocalKanbanMode()) && !!organizationId;

  const { data, isLoading, error } = useShape(
    PROJECTS_SHAPE,
    { organization_id: organizationId || '' },
    { enabled }
  );

  return {
    data,
    isLoading,
    isError: !!error,
    error,
  };
}
