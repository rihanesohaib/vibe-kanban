import { createFileRoute } from '@tanstack/react-router';
import { ControlCenterPage } from '@/pages/control-center/ControlCenterPage';

export const Route = createFileRoute('/_app/control-center')({
  component: ControlCenterPage,
});
