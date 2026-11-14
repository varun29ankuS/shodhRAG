/**
 * The Library as on the computer: the linked folders, then their folders and
 * files at any depth, with a path of breadcrumbs back up. Files show their
 * first page, rendered on the computer; a tap opens the Reader, zooming
 * from the page.
 */
import { Fragment, type MouseEvent } from 'react';
import { motion } from 'framer-motion';
import { ChevronRight, Folder } from 'lucide-react';
import { TopBar } from '../components/Bars';
import { NotConnected } from '../components/Connection';
import { PageThumb } from '../components/PageThumb';
import { useLink, useNav } from '../lib/context';
import { libraryFolders, type FolderEntry } from '../lib/desktop';
import { pushIn, viewIn } from '../lib/motion';
import type { Folder as Opened } from '../lib/nav';
import { useRemote } from '../lib/remote';

export function Library() {
  const { connected } = useLink();
  const { nav, dispatch } = useNav();
  const here: Opened | null = nav.library[nav.library.length - 1] ?? null;
  const listing = useRemote(`library:${here?.path ?? ''}`, desktop => libraryFolders(desktop, here?.path ?? null));
  const data = listing.data;

  const openFile = (file: FolderEntry, event: MouseEvent<HTMLElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    dispatch({
      type: 'read',
      target: {
        path: file.path,
        name: file.name,
        page: null,
        passage: null,
        back: here?.name ?? 'Library',
        origin: { x: rect.left, y: rect.top, width: rect.width, height: rect.height },
      },
    });
  };

  return (
    <motion.div key={here?.path ?? ''} className="view" variants={here ? pushIn : viewIn} initial="hidden" animate="shown" exit="gone">
      <div className="scroll">
        <TopBar brand={false} />
        <nav className="crumbs" aria-label="Folder path">
          {[{ name: 'Library', path: '' }, ...nav.library].map((crumb, depth, all) => (
            <Fragment key={`${depth}:${crumb.path}`}>
              <button
                type="button"
                className="crumb"
                aria-current={depth === all.length - 1 ? 'page' : undefined}
                onClick={() => dispatch({ type: 'crumb', depth })}
              >
                {crumb.name}
              </button>
              {depth < all.length - 1 && <ChevronRight aria-hidden="true" className="crumb-sep" />}
            </Fragment>
          ))}
        </nav>
        <h1 className="title serif pad">{here?.name ?? 'Library'}</h1>

        {data === undefined && !connected && <NotConnected what="your folders and files" />}
        {data === undefined && connected && listing.error === null && (
          <div className="skeleton-lines" aria-busy="true" aria-label="Loading the folder">
            {[70, 55, 62].map(width => (
              <div key={width} className="skeleton" style={{ width: `${width}%`, height: 48, borderRadius: 14 }} />
            ))}
          </div>
        )}
        {listing.error !== null && data === undefined && (
          <p className="panel-text pad" style={{ marginTop: 16 }}>
            {listing.error}
          </p>
        )}
        {data !== undefined && data.folders.length === 0 && data.files.length === 0 && (
          <section className="panel panel-quiet">
            <p className="panel-title">{here ? 'Nothing here yet' : 'No folders linked yet'}</p>
            <p className="panel-text">
              {here
                ? 'Shodh lists the files it has indexed in this folder. Files still being indexed show when they are done.'
                : 'Link a folder in Shodh on your computer (Library, then Add folder), and it shows here.'}
            </p>
          </section>
        )}

        {data !== undefined && data.folders.length > 0 && (
          <div className="rows" style={{ marginTop: 12 }}>
            {data.folders.map(folder => (
              <button
                key={folder.path}
                type="button"
                className="row"
                onClick={() => dispatch({ type: 'folder', folder: { path: folder.path, name: folder.name } })}
              >
                <span className="row-icon">
                  <Folder aria-hidden="true" className="icon" />
                </span>
                <span className="row-text">
                  <span className="row-title" style={{ fontSize: 16 }}>
                    {folder.name}
                  </span>
                  {!here && <span className="row-sub">Linked folder</span>}
                </span>
                <ChevronRight aria-hidden="true" className="icon-small faint" />
              </button>
            ))}
          </div>
        )}

        {data !== undefined && data.files.length > 0 && (
          <div className="files">
            {data.files.map(file => (
              <button key={file.path} type="button" className="tile" aria-label={`Open ${file.name}`} onClick={event => openFile(file, event)}>
                <PageThumb path={file.path} />
                <span className="tile-name">{file.name}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    </motion.div>
  );
}
