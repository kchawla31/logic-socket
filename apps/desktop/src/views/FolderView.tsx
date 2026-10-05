import { useEffect, useState } from 'react';

import { CodeEditor, KeyValueEditor } from '../components/editors';
import { Empty, Input, Tabs } from '../components/ui';
import { api, type Folder } from '../lib/api';
import { AuthEditor } from './AuthEditor';
import { useAutosave } from './RequestView';

export function FolderView({ id }: { id: string }) {
  const [folder, setFolder] = useAutosave<Folder>(id, api.folderUpdate);
  const [tab, setTab] = useState('environment');
  const [envText, setEnvText] = useState('{}');
  const [envError, setEnvError] = useState<string | null>(null);

  useEffect(() => {
    if (folder) setEnvText(JSON.stringify(folder.environment ?? {}, null, 2));
    // only when switching folders
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [folder?.id]);

  if (!folder) return <Empty title="Loading…" />;
  const update = (p: Partial<Folder>) => setFolder({ ...folder, ...p });

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-app p-2">
        <Input className="max-w-md font-medium" value={folder.name} onChange={e => update({ name: e.target.value })} aria-label="Folder name" />
        <span className="text-[12px] text-muted">Settings here apply to every request inside this folder.</span>
      </div>
      <Tabs
        value={tab}
        onChange={setTab}
        tabs={[
          { id: 'environment', label: 'Environment', count: Object.keys(folder.environment ?? {}).length },
          { id: 'headers', label: 'Headers', count: folder.headers.filter(h => !h.disabled).length },
          { id: 'auth', label: 'Auth', dot: folder.authentication.type !== 'inherit' },
          { id: 'docs', label: 'Docs' },
        ]}
      />
      <div className="min-h-0 flex-1 overflow-auto">
        {tab === 'environment' && (
          <div className="flex h-full flex-col">
            <div className="shrink-0 border-b border-app px-3 py-1.5 text-[12px] text-muted">
              Folder variables override the base and active environment for requests in this folder. JSON object.
              {envError && <span className="ml-2 text-rose-600 dark:text-rose-400">⚠ {envError}</span>}
            </div>
            <CodeEditor
              value={envText}
              onChange={t => {
                setEnvText(t);
                try {
                  const v = JSON.parse(t || '{}');
                  if (!v || typeof v !== 'object' || Array.isArray(v)) throw new Error('must be a JSON object');
                  setEnvError(null);
                  update({ environment: v });
                } catch (e) {
                  setEnvError((e as Error).message);
                }
              }}
            />
          </div>
        )}
        {tab === 'headers' && <KeyValueEditor items={folder.headers} onChange={headers => update({ headers })} contextId={id} namePlaceholder="Header" />}
        {tab === 'auth' && <AuthEditor auth={folder.authentication} onChange={authentication => update({ authentication })} contextId={id} />}
        {tab === 'docs' && (
          <textarea
            value={folder.description}
            onChange={e => update({ description: e.target.value })}
            placeholder="Describe this folder (Markdown)…"
            className="h-full w-full resize-none bg-app p-3 outline-none"
          />
        )}
      </div>
    </div>
  );
}
