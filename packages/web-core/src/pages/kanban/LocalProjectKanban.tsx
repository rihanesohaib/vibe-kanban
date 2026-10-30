import { ProjectKanban } from '@/pages/kanban/ProjectKanban';
import { RequireRemote } from '@/shared/components/RequireRemote';

export function LocalProjectKanban() {
  return (
    <RequireRemote>
      <ProjectKanban />
    </RequireRemote>
  );
}
