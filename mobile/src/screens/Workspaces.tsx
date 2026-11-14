/**
 * Workspaces: the user's workspaces as the computer lists them. Opening one
 * (its sources, instructions and chats) comes to the phone later; for now
 * the list says what each holds.
 */
import { motion } from 'framer-motion';
import { Layers } from 'lucide-react';
import { TopBar } from '../components/Bars';
import { NotConnected } from '../components/Connection';
import { useLink } from '../lib/context';
import { workspacesList } from '../lib/desktop';
import { viewIn } from '../lib/motion';
import { useRemote } from '../lib/remote';
import { ago } from '../lib/time';

export function Workspaces({ now = () => new Date() }: { now?: () => Date }) {
  const { connected } = useLink();
  const workspaces = useRemote('workspaces', workspacesList);
  const list = workspaces.data;
  const today = now();
  return (
    <motion.div className="view" variants={viewIn} initial="hidden" animate="shown" exit="gone">
      <div className="scroll">
        <TopBar brand={false} />
        <h1 className="title serif pad">Workspaces</h1>
        {list === undefined && !connected && <NotConnected what="your workspaces" />}
        {list === undefined && connected && (
          <div className="skeleton-lines" aria-busy="true" aria-label="Loading your workspaces">
            {[76, 60, 68].map(width => (
              <div key={width} className="skeleton" style={{ width: `${width}%`, height: 52, borderRadius: 14 }} />
            ))}
          </div>
        )}
        {list !== undefined && list.length === 0 && (
          <section className="panel panel-quiet">
            <p className="panel-title">No workspaces yet</p>
            <p className="panel-text">Make one in Shodh on your computer: Workspaces, then New workspace.</p>
          </section>
        )}
        {list !== undefined && list.length > 0 && (
          <>
            <div className="label">Your workspaces</div>
            <div className="card">
              {list.map(workspace => (
                <div key={workspace.id} className="row row-divided">
                  <span className="row-icon">
                    <Layers aria-hidden="true" className="icon-small" />
                  </span>
                  <span className="row-text">
                    <span className="row-title">{workspace.name}</span>
                    <span className="row-sub">
                      {[
                        `${workspace.chatCount} ${workspace.chatCount === 1 ? 'conversation' : 'conversations'}`,
                        workspace.lastActiveAt ? `last used ${ago(workspace.lastActiveAt, today).toLowerCase()}` : null,
                      ]
                        .filter(Boolean)
                        .join(', ')}
                    </span>
                  </span>
                </div>
              ))}
            </div>
            <p className="panel-text pad" style={{ marginTop: 14 }}>
              Opening a workspace (its sources, instructions and chats) is coming to the phone soon.
            </p>
          </>
        )}
      </div>
    </motion.div>
  );
}
