import { useEffect, useState } from 'react';
import { notify } from '../../lib/notify';
import { agentItemKey } from '../../lib/agentHighlight';
import { SwitchRow } from '../../components/PrivacySettings';
import { codeIntelApi, errorText } from './api';
import type { CodeSkill } from './api';

/** "obra/superpowers (MIT) · google/elephant-goldfish-model (Apache-2.0)". */
function sourcesLine(skill: CodeSkill): string {
  return skill.sources.map(s => `${s.project} (${s.license})`).join(' · ');
}

/**
 * Settings → Code: Shodh's building skills for Code mode, each on or off, and
 * whether Code conversations shorten command output for the model by default.
 * The agent can list both but never change them.
 */
export default function CodeSkillsSettings() {
  const [skills, setSkills] = useState<CodeSkill[] | null>(null);
  const [shortenByDefault, setShortenByDefault] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    codeIntelApi
      .skills()
      .then(next => {
        if (active) setSkills(next);
      })
      .catch(err => {
        if (active) setError(errorText(err));
      });
    codeIntelApi
      .total()
      .then(total => {
        if (active) setShortenByDefault(total.defaultOn);
      })
      .catch(err => {
        if (active) setError(errorText(err));
      });
    return () => {
      active = false;
    };
  }, []);

  const toggle = (skill: CodeSkill, enabled: boolean) => {
    setSaving(skill.name);
    codeIntelApi
      .setSkill(skill.name, enabled)
      .then(next => {
        setSkills(next);
        notify.success(`${skill.title} is ${enabled ? 'on' : 'off'}`, {
          description: 'Code conversations started from now on use the change.',
        });
      })
      .catch(err => notify.error(`Could not change ${skill.title}`, { description: errorText(err) }))
      .finally(() => setSaving(null));
  };

  const toggleDefault = (enabled: boolean) => {
    setSaving('default');
    codeIntelApi
      .setDefaultShortening(enabled)
      .then(total => setShortenByDefault(total.defaultOn))
      .catch(err => notify.error('Could not change shorter command output', { description: errorText(err) }))
      .finally(() => setSaving(null));
  };

  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-col gap-1">
        <h3 className="m-0 text-body font-semibold text-shodh-text">Building skills</h3>
        <p className="m-0 text-small text-shodh-text-muted">
          Instructions the assistant follows in Code mode when a task calls for one: plan first, test first, find the
          cause of a failure, check the work before calling it done, and have it reviewed. They never change the mode
          or what needs your approval. A skill you turn off is not followed.
        </p>
      </div>
      {error && (
        <p role="alert" className="m-0 text-small text-shodh-error">
          {error}
        </p>
      )}
      {skills && (
        <ul className="m-0 p-0 list-none flex flex-col gap-4">
          {skills.map(skill => (
            <li key={skill.name} data-agent-item={agentItemKey('setting', `code-skill-${skill.name}`)} className="flex flex-col gap-1">
              <SwitchRow
                label={skill.title}
                description={skill.summary}
                checked={skill.enabled}
                disabled={saving !== null}
                onChange={enabled => toggle(skill, enabled)}
              />
              <p className="m-0 text-caption text-shodh-text-faint">Written from {sourcesLine(skill)}</p>
            </li>
          ))}
        </ul>
      )}
      <div className="flex flex-col gap-3 border-t border-shodh-border-subtle pt-4">
        <SwitchRow
          label="Shorten command output"
          description="In Code conversations the model reads command output without passing-test lines, and, after a successful run, without build progress. Errors, warnings and failing tests are kept, the model can read the full output, and you always see it. Each conversation can change this from its message box; this is what new ones start with."
          checked={shortenByDefault ?? true}
          disabled={saving !== null || shortenByDefault === null}
          onChange={toggleDefault}
        />
        <p className="m-0 text-small text-shodh-text-muted">The tokens saved show in each Code conversation and, in total, in Activity.</p>
      </div>
    </div>
  );
}
