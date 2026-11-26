/**
 * "Update Shodh on your phone": the computer runs a newer Shodh, so the phone
 * looks for the matching app in Shodh's release, and one tap downloads it,
 * checks it is Shodh's own (its signature and checksum) and hands it to
 * Android, which asks the user to confirm. An iPhone is updated by TestFlight
 * or the App Store: there the check says where, and nothing is downloaded.
 */
import { useEffect, useState, type ReactNode } from 'react';
import { Download, LoaderCircle, RefreshCw, ShieldCheck, TriangleAlert } from 'lucide-react';
import type { UpdateCheck } from '../lib/device';
import { errorText } from '../lib/link';
import { SYSTEM } from '../lib/platform';
import { useDevice } from '../lib/queue';
import { version } from '../../package.json';

type Stage =
  | { kind: 'checking' }
  | { kind: 'found'; check: UpdateCheck }
  | { kind: 'installing' }
  | { kind: 'confirm' }
  | { kind: 'allow' }
  | { kind: 'failed'; message: string };

/** Megabytes, for the button ("24 MB"). */
export function megabytes(bytes: number): string {
  return `${Math.max(1, Math.round(bytes / 1_048_576))} MB`;
}

export function UpdatePanel({ desktop }: { desktop: string }) {
  const { updater } = useDevice();
  const [stage, setStage] = useState<Stage>({ kind: 'checking' });

  const check = () => {
    setStage({ kind: 'checking' });
    updater
      .check()
      .then(found => setStage({ kind: 'found', check: found }))
      .catch(e => setStage({ kind: 'failed', message: errorText(e) }));
  };
  useEffect(check, [updater]);

  const install = () => {
    setStage({ kind: 'installing' });
    updater
      .install()
      .then(started => setStage({ kind: started ? 'confirm' : 'allow' }))
      .catch(e => setStage({ kind: 'failed', message: errorText(e) }));
  };

  let body: ReactNode = null;
  switch (stage.kind) {
    case 'checking':
      body = (
        <p className="panel-text" role="status">
          <LoaderCircle aria-hidden="true" className="icon-small spin" /> {`Looking for Shodh ${desktop} for ${SYSTEM}…`}
        </p>
      );
      break;
    case 'found':
      body =
        stage.check.state === 'ready' ? (
          <button type="button" className="button button-primary" onClick={install}>
            <Download aria-hidden="true" className="icon-small" />
            {`Update to ${stage.check.version} (${megabytes(stage.check.size)})`}
          </button>
        ) : (
          <>
            <p className="panel-text">{stage.check.message}</p>
            <button type="button" className="button" onClick={check}>
              <RefreshCw aria-hidden="true" className="icon-small" />
              Look again
            </button>
          </>
        );
      break;
    case 'installing':
      body = (
        <p className="panel-text" role="status">
          <LoaderCircle aria-hidden="true" className="icon-small spin" /> Downloading the update and checking it is Shodh's own…
        </p>
      );
      break;
    case 'confirm':
      body = (
        <p className="panel-text" role="status">
          <ShieldCheck aria-hidden="true" className="icon-small" /> The update is checked. Android asks you to confirm it.
        </p>
      );
      break;
    case 'allow':
      body = (
        <>
          <p className="panel-text">Allow Shodh to install updates in the settings that opened, then come back and tap Update.</p>
          <button type="button" className="button button-primary" onClick={install}>
            <Download aria-hidden="true" className="icon-small" />
            Update
          </button>
        </>
      );
      break;
    case 'failed':
      body = (
        <>
          <p className="panel-text">{stage.message}</p>
          <button type="button" className="button" onClick={check}>
            <RefreshCw aria-hidden="true" className="icon-small" />
            Try again
          </button>
        </>
      );
      break;
  }

  return (
    <section className="panel">
      <span className="panel-icon">
        <TriangleAlert aria-hidden="true" className="icon" />
      </span>
      <p className="panel-title">Update Shodh on your phone</p>
      <p className="panel-text">{`Your computer runs Shodh ${desktop}, and this phone runs ${version}. They must be the same version.`}</p>
      {body}
    </section>
  );
}
