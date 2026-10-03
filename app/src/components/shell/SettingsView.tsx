import React, { useState } from 'react';
import LLMSettings from '../../LLMSettings';
import SearchSettings from '../SearchSettings';
import DataManagement from '../DataManagement';
import PrivacySettings from '../PrivacySettings';
import BackgroundSettings from '../BackgroundSettings';
import MemorySettings from '../MemorySettings';
import { cn } from '../../lib/utils';

export type SettingsSection = 'models' | 'search' | 'general' | 'privacy' | 'memory' | 'data';

const SECTIONS: { id: SettingsSection; label: string; description: string }[] = [
  {
    id: 'models',
    label: 'Models',
    description: 'Choose which model answers questions, and where your questions and retrieved passages are allowed to go.',
  },
  {
    id: 'search',
    label: 'Search',
    description: 'Choose how many passages each document search retrieves.',
  },
  {
    id: 'general',
    label: 'General',
    description: 'How Shodh runs when its window is closed, so task reminders keep ringing.',
  },
  {
    id: 'privacy',
    label: 'Privacy',
    description: 'Decide whether anything may leave this computer, and whether the assistant may use the web.',
  },
  {
    id: 'memory',
    label: 'Memory',
    description: 'What Shodh remembers about you: how strong each memory is, where it came from and how it changed. Edit, pin, forget or export them.',
  },
  {
    id: 'data',
    label: 'Data',
    description: 'Inspect the local index, remove sources, or reset stored data on this computer.',
  },
];

type SearchSettingsProps = React.ComponentProps<typeof SearchSettings>;
type DataManagementProps = React.ComponentProps<typeof DataManagement>;

interface SettingsViewProps {
  /** Re-check the active LLM after its configuration changes. */
  onModelStatusChange: () => void;
  /** Called when the model configuration panel is dismissed. */
  onCloseModelSettings: () => void;
  searchConfig: SearchSettingsProps['config'];
  onUpdateSearchConfig: SearchSettingsProps['onUpdate'];
  onResetSearchConfig: SearchSettingsProps['onReset'];
  sources: DataManagementProps['sources'];
  onRemoveSource: DataManagementProps['onRemoveSource'];
  onSourcesCleared: DataManagementProps['onSourcesCleared'];
  /** Saved conversations (titles for memory sources). */
  conversations: readonly { id: string; title: string }[];
  /** Open a conversation in Ask. */
  onOpenConversation: (id: string) => void;
}

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background';

export default function SettingsView({
  onModelStatusChange,
  onCloseModelSettings,
  searchConfig,
  onUpdateSearchConfig,
  onResetSearchConfig,
  sources,
  onRemoveSource,
  onSourcesCleared,
  conversations,
  onOpenConversation,
}: SettingsViewProps) {
  const [section, setSection] = useState<SettingsSection>('models');
  const active = SECTIONS.find(s => s.id === section) ?? SECTIONS[0];

  return (
    <div className="h-full flex bg-shodh-ground text-shodh-text">
      <nav
        aria-label="Settings sections"
        className="w-[220px] shrink-0 border-r border-shodh-border-subtle px-3 py-7 flex flex-col gap-0.5"
      >
        <h1 className="px-3 pb-2.5 text-[18px] font-bold">Settings</h1>
        <ul className="flex flex-col gap-0.5">
          {SECTIONS.map(s => {
            const isActive = s.id === section;
            return (
              <li key={s.id}>
                <button
                  type="button"
                  onClick={() => setSection(s.id)}
                  aria-current={isActive ? 'page' : undefined}
                  className={cn(
                    'w-full h-9 px-3 rounded-lg text-left text-[13.5px] transition-colors duration-micro',
                    isActive
                      ? 'bg-shodh-raised-2 text-shodh-text font-semibold'
                      : 'text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text',
                    FOCUS_RING
                  )}
                >
                  {s.label}
                </button>
              </li>
            );
          })}
        </ul>
      </nav>

      {section === 'models' ? (
        <section aria-labelledby="settings-section-heading" className="flex-1 min-w-0 overflow-y-auto">
          <div className="max-w-[820px] px-10 py-8 flex flex-col gap-7">
            <header className="flex flex-col gap-1.5">
              <h2 id="settings-section-heading" className="m-0 text-2xl font-bold">
                {active.label}
              </h2>
              <p className="text-sm text-shodh-text-muted">{active.description}</p>
            </header>
            <LLMSettings embedded onClose={onCloseModelSettings} onStatusChange={onModelStatusChange} />
          </div>
        </section>
      ) : (
        <section aria-labelledby="settings-section-heading" className="flex-1 min-w-0 overflow-y-auto">
          <div className="max-w-[820px] px-10 py-8 flex flex-col gap-7">
            <header className="flex flex-col gap-1.5">
              <h2 id="settings-section-heading" className="m-0 text-2xl font-bold">
                {active.label}
              </h2>
              <p className="text-sm text-shodh-text-muted">{active.description}</p>
            </header>

            <section className="p-5 rounded-[14px] bg-shodh-surface border border-shodh-border">
              {section === 'search' ? (
                <SearchSettings
                  config={searchConfig}
                  onUpdate={onUpdateSearchConfig}
                  onReset={onResetSearchConfig}
                />
              ) : section === 'general' ? (
                <BackgroundSettings />
              ) : section === 'privacy' ? (
                <PrivacySettings />
              ) : section === 'memory' ? (
                <MemorySettings
                  conversationTitle={id => conversations.find(c => c.id === id)?.title}
                  onOpenConversation={onOpenConversation}
                  sourceName={id => sources.find(s => s.id === id)?.name}
                />
              ) : (
                <DataManagement
                  sources={sources}
                  onRemoveSource={onRemoveSource}
                  onSourcesCleared={onSourcesCleared}
                />
              )}
            </section>
          </div>
        </section>
      )}
    </div>
  );
}
