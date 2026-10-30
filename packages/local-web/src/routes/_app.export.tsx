import { createFileRoute } from '@tanstack/react-router';
import { ExportPageContainer } from '@/pages/export/ExportPage';
import { RequireRemote } from '@/shared/components/RequireRemote';

function ExportRouteComponent() {
  return (
    <RequireRemote>
      <ExportPageContainer />
    </RequireRemote>
  );
}

export const Route = createFileRoute('/_app/export')({
  component: ExportRouteComponent,
});
