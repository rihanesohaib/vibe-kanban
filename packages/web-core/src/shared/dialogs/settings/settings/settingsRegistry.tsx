import {
  GearIcon,
  GitBranchIcon,
  BuildingsIcon,
  CloudIcon,
  CpuIcon,
  PlugIcon,
  BroadcastIcon,
} from '@phosphor-icons/react';
import type { Icon } from '@phosphor-icons/react';
import { GeneralSettingsSection } from './GeneralSettingsSection';
import { ReposSettingsSection } from './ReposSettingsSection';
import { OrganizationsSettingsSection } from './OrganizationsSettingsSection';
import { RemoteProjectsSettingsSection } from './RemoteProjectsSettingsSection';
import { AgentsSettingsSection } from './AgentsSettingsSection';
import { McpSettingsSection } from './McpSettingsSection';
import { RelaySettingsSectionContent } from './RelaySettingsSection';
import { isRemoteEnabled } from '@/shared/lib/remoteApi';

export type SettingsSectionType =
  | 'general'
  | 'repos'
  | 'organizations'
  | 'remote-projects'
  | 'agents'
  | 'mcp'
  | 'relay';

export type SettingsSectionGroup = 'host' | 'universal';

export type SettingsSectionInitialState = {
  general: undefined;
  repos: { repoId?: string } | undefined;
  organizations: { organizationId?: string } | undefined;
  'remote-projects':
    { organizationId?: string; projectId?: string } | undefined;
  agents: { executor?: string; variant?: string } | undefined;
  mcp: undefined;
  relay: { hostId?: string } | undefined;
};

export interface SettingsSectionDefinition {
  id: SettingsSectionType;
  icon: Icon;
  group: SettingsSectionGroup;
  requiresRemote?: boolean;
}

export const SETTINGS_SECTION_DEFINITIONS: SettingsSectionDefinition[] = [
  { id: 'general', icon: GearIcon, group: 'host' },
  { id: 'repos', icon: GitBranchIcon, group: 'host' },
  { id: 'agents', icon: CpuIcon, group: 'host' },
  { id: 'mcp', icon: PlugIcon, group: 'host' },
  {
    id: 'organizations',
    icon: BuildingsIcon,
    group: 'universal',
    requiresRemote: true,
  },
  {
    id: 'remote-projects',
    icon: CloudIcon,
    group: 'universal',
  },
  {
    id: 'relay',
    icon: BroadcastIcon,
    group: 'universal',
    requiresRemote: true,
  },
];

export function getAvailableSettingsSections(): SettingsSectionDefinition[] {
  const remoteEnabled = isRemoteEnabled();
  return SETTINGS_SECTION_DEFINITIONS.filter(
    (section) => remoteEnabled || !section.requiresRemote
  );
}

export function isHostSpecificSettingsSection(
  type: SettingsSectionType
): boolean {
  return (
    SETTINGS_SECTION_DEFINITIONS.find((section) => section.id === type)
      ?.group === 'host'
  );
}

export function renderSettingsSection(
  type: SettingsSectionType,
  initialState?: SettingsSectionInitialState[SettingsSectionType],
  onClose?: () => void
) {
  switch (type) {
    case 'general':
      return <GeneralSettingsSection />;
    case 'repos':
      return (
        <ReposSettingsSection
          initialState={initialState as SettingsSectionInitialState['repos']}
        />
      );
    case 'organizations':
      return <OrganizationsSettingsSection />;
    case 'remote-projects':
      return (
        <RemoteProjectsSettingsSection
          initialState={
            initialState as SettingsSectionInitialState['remote-projects']
          }
        />
      );
    case 'agents':
      return <AgentsSettingsSection />;
    case 'mcp':
      return <McpSettingsSection />;
    case 'relay':
      return (
        <RelaySettingsSectionContent
          initialState={initialState as SettingsSectionInitialState['relay']}
          onClose={onClose}
        />
      );
    default:
      return <GeneralSettingsSection />;
  }
}
