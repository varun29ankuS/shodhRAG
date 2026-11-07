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
 * Settings → Code: Shodh's building skills for Code mode, each on or off. The
 * agent loads one when a task calls for it; it can list them but never turn
 * one on or off.
 */
export default function CodeSkillsSettings() {
  const [skills, setSkills] = useState<CodeSkill[] | null>(null);
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

  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-col gap-1">
        <h3 className="m-0 text-body font-semibold text-shodh-text">Building skills</h3>
        <p className="m-0 text-small text-shodh-text-muted">
          Instructions the assistant follows in Code mode when a task calls for one: plan first, test first, find the
          cause of a failure, check the work before calling it done, and have it reviewed. They never change the mode
          or what needs your approval.
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
      <div className="flex flex-col gap-1 border-t border-shodh-border-subtle pt-4">
        <h3 className="m-0 text-body font-semibold text-shodh-text">Shorter command output</h3>
        <p className="m-0 text-small text-shodh-text-muted">
          In Code mode the model reads command output without build progress and passing tests; every error and failing
          test is kept, and you always see the full output. The tokens saved show in each Code conversation&apos;s
          message box, where you can turn this off for that conversation, and in total in Activity.
        </p>
      </div>
    </div>
  );
}
