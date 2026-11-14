/**
 * What a Code or Creator answer did, from its saved transcript (lib/code.ts):
 * the files it changed, and the commands it ran with their output. Shown to
 * read only: nothing here approves or runs anything; a step still waiting is
 * approved on the computer.
 */
import { Check, FilePenLine, Laptop, Sparkles, SquareTerminal, X } from 'lucide-react';
import { fileName } from '../lib/answer';
import { commandEndWords, commandOk, type CodeWork } from '../lib/code';

/** The note at the top of a Code conversation. */
export const CODE_NOTE = 'Code changes are approved on your computer.';

/** What a Creator conversation says in place of its preview. */
export const CREATOR_PREVIEW_ELSEWHERE = 'Open on your computer to see the preview';

export function CodeNote() {
  return (
    <p className="code-note" role="note">
      <Laptop aria-hidden="true" className="icon-small" />
      <span>{CODE_NOTE}</span>
    </p>
  );
}

export function CodeWorkView({ work }: { work: CodeWork }) {
  if (work.files.length === 0 && work.commands.length === 0 && work.waiting.length === 0) return null;
  return (
    <div className="code-work">
      {work.files.length > 0 && (
        <section aria-label="Files changed">
          <div className="code-label">{work.files.length === 1 ? '1 file changed' : `${work.files.length} files changed`}</div>
          <ul className="code-files">
            {work.files.map(file => (
              <li key={file.path} className="code-file">
                <FilePenLine aria-hidden="true" className="icon-small code-file-icon" />
                <span className="code-file-text">
                  <span className="code-file-name mono">{fileName(file.path)}</span>
                  <span className="code-file-path mono">{file.path}</span>
                </span>
                <span className="code-file-what">{file.written ? 'Written' : file.changes === 1 ? 'Edited' : `${file.changes} edits`}</span>
              </li>
            ))}
          </ul>
        </section>
      )}
      {work.commands.length > 0 && (
        <section aria-label="Commands run">
          <div className="code-label">{work.commands.length === 1 ? '1 command' : `${work.commands.length} commands`}</div>
          {work.commands.map(command => {
            const ok = commandOk(command);
            return (
              <div key={command.stepId} className="command">
                <div className="command-head">
                  <SquareTerminal aria-hidden="true" className="icon-small faint" />
                  <code className="command-line mono">{command.command}</code>
                </div>
                <div className={`command-end ${ok ? 'command-ok' : 'command-bad'}`}>
                  {ok ? <Check aria-hidden="true" className="icon-small" /> : <X aria-hidden="true" className="icon-small" />}
                  <span>{commandEndWords(command)}</span>
                </div>
                {command.note && <p className="command-note">{command.note}</p>}
                {command.output !== '' && (
                  <pre className="command-output mono" aria-label={`Output of ${command.command}`}>
                    {command.outputCut ? `…\n${command.output}` : command.output}
                  </pre>
                )}
              </div>
            );
          })}
        </section>
      )}
      {work.waiting.map(step => (
        <p key={step.stepId} className="on-computer">
          <Laptop aria-hidden="true" className="icon-small" />
          <span>{`Waiting for approval on your computer: ${step.label}`}</span>
        </p>
      ))}
    </div>
  );
}

/**
 * Creator's place for its preview. The saved conversation holds no preview of
 * what was built (the computer shows it in a window of its own, served from
 * the creation's folder), so the phone says where to see it, with what the
 * build recorded: its summary and the files it wrote.
 */
export function CreatorPreview({ work }: { work: CodeWork }) {
  return (
    <div className="creator-preview">
      <div className="creator-preview-head">
        <Sparkles aria-hidden="true" className="icon-small" />
        <span>{CREATOR_PREVIEW_ELSEWHERE}</span>
      </div>
      {work.finished && <p className="creator-summary serif">{work.finished}</p>}
      {work.files.length > 0 && (
        <p className="creator-files">{work.files.length === 1 ? '1 file made' : `${work.files.length} files made`}</p>
      )}
    </div>
  );
}
