import { useMemo, type ReactNode } from 'react';
import {
  AuthContext,
  type AuthContextValue,
} from '@/shared/hooks/auth/useAuth';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { isLocalKanbanMode, LOCAL_USER_ID } from '@/shared/lib/remoteApi';

interface LocalAuthProviderProps {
  children: ReactNode;
}

export function LocalAuthProvider({ children }: LocalAuthProviderProps) {
  const { loginStatus } = useUserSystem();

  const value = useMemo<AuthContextValue>(() => {
    const isSignedIn = loginStatus?.status === 'loggedin';
    const signedInUserId =
      loginStatus?.status === 'loggedin'
        ? (loginStatus.profile?.user_id ?? null)
        : null;
    return {
      isSignedIn,
      isLoaded: loginStatus !== null,
      // The local kanban has one user who owns every workspace and comment.
      userId: signedInUserId ?? (isLocalKanbanMode() ? LOCAL_USER_ID : null),
    };
  }, [loginStatus]);

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}
