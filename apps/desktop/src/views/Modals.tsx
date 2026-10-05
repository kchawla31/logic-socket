import { Check, Plus } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';

import { CodeEditor } from '../components/editors';
import { Button, Input, Modal, Select, Toggle, useDialog, useToast } from '../components/ui';
import { api, type EnvList, type Environment, errorText, type Settings } from '../lib/api';
import { cn } from '../lib/utils';

export function EnvironmentModal({ open, onClose, workspaceId }: { open: boolean; onClose: () => void; workspaceId: string }) {
  const toast = useToast();
  const dialog = useDialog();
  const [list, setList] = useState<EnvList | null>(null);
  const [selId, setSelId] = useState<string | null>(null);
  const [text, setText] = useState('');
  const [err, setErr] = useState<string | null>(null);

  const reload = () => api.envList(workspaceId).then(setList);
  useEffect(() => {
    if (open) reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, workspaceId]);

  const all = useMemo(() => (list ? [list.base, ...list.subs] : []), [list]);
  const sel = all.find(e => e.id === selId) ?? all[0];
  useEffect(() => {
    if (sel) setText(JSON.stringify(sel.data, null, 2));
    setErr(null);
  }, [sel?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  const save = async (env: Environment, t: string) => {
    try {
      const data = JSON.parse(t || '{}');
      if (!data || typeof data !== 'object' || Array.isArray(data)) throw new Error('must be a JSON object');
      setErr(null);
      await api.envUpdate({ ...env, data });
    } catch (e) {
      setErr(errorText(e));
    }
  };

  return (
    <Modal open={open} onClose={onClose} title="Environments" width="max-w-4xl">
      {list && sel && (
        <div className="flex h-[60vh]">
          <div className="flex w-56 shrink-0 flex-col border-r border-app bg-subtle p-2">
            {all.map((e, i) => (
              <button
                key={e.id}
                onClick={() => setSelId(e.id)}
                className={cn('mb-0.5 flex items-center gap-2 rounded-md px-2.5 py-1.5 text-left', sel.id === e.id ? 'bg-accent-soft' : 'hover:bg-muted')}
              >
                <span className="min-w-0 flex-1 truncate">{e.name}</span>
                {i === 0 && <span className="text-[10.5px] text-muted">base</span>}
                {list.activeId === e.id && <Check className="size-3.5 text-emerald-500" />}
              </button>
            ))}
            <Button
              size="sm"
              variant="ghost"
              className="mt-1 justify-start"
              onClick={async () => {
                const name = await dialog.prompt('New environment', 'Staging');
                if (!name) return;
                const e = await api.envCreate(workspaceId, name).catch(x => (toast(errorText(x), 'error'), null));
                if (e) {
                  await reload();
                  setSelId(e.id);
                }
              }}
            >
              <Plus className="size-3.5" /> New environment
            </Button>
          </div>
          <div className="flex min-w-0 flex-1 flex-col">
            <div className="flex shrink-0 items-center gap-2 border-b border-app p-2">
              <Input
                className="max-w-xs font-medium"
                value={sel.name}
                disabled={sel.id === list.base.id}
                onChange={e => setList({ ...list, subs: list.subs.map(s => (s.id === sel.id ? { ...s, name: e.target.value } : s)) })}
                onBlur={() => api.envUpdate({ ...sel, data: JSON.parse(text || '{}') }).catch(() => {})}
              />
              <div className="flex-1" />
              {sel.id !== list.base.id &&
                (list.activeId === sel.id ? (
                  <Button size="sm" onClick={() => api.envSetActive(workspaceId, null).then(reload)}>
                    Deactivate
                  </Button>
                ) : (
                  <Button size="sm" variant="primary" onClick={() => api.envSetActive(workspaceId, sel.id).then(reload)}>
                    Use this environment
                  </Button>
                ))}
              {sel.id !== list.base.id && (
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={async () => {
                    if (!(await dialog.confirm(`Delete environment "${sel.name}"?`, { danger: true, confirmLabel: 'Delete' }))) return;
                    await api.itemDelete(sel.id);
                    setSelId(null);
                    reload();
                  }}
                >
                  Delete
                </Button>
              )}
            </div>
            <div className="shrink-0 border-b border-app px-3 py-1.5 text-[12px] text-muted">
              {sel.id === list.base.id
                ? 'Base variables are always available. The active sub-environment overrides them.'
                : 'Overrides the base environment while active.'}{' '}
              Reference with <code className="font-mono">{'{{ _.name }}'}</code>; values may reference other variables.
              {err && <span className="ml-2 text-rose-600 dark:text-rose-400">⚠ {err}</span>}
            </div>
            <CodeEditor
              value={text}
              onChange={t => {
                setText(t);
                save(sel, t);
              }}
            />
          </div>
        </div>
      )}
    </Modal>
  );
}

export function SettingsModal({
  open,
  onClose,
  theme,
  setTheme,
}: {
  open: boolean;
  onClose: () => void;
  theme: string;
  setTheme: (t: string) => void;
}) {
  const [s, setS] = useState<Settings | null>(null);
  useEffect(() => {
    if (open) api.settingsGet().then(setS);
  }, [open]);
  const update = (p: Partial<Settings>) => {
    if (!s) return;
    const next = { ...s, ...p };
    setS(next);
    api.settingsUpdate(next);
  };
  return (
    <Modal open={open} onClose={onClose} title="Settings" width="max-w-lg">
      {s && (
        <div className="flex flex-col gap-4 p-5">
          <Row label="Theme">
            <Select value={theme} onChange={e => setTheme(e.target.value)}>
              <option value="system">System</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </Select>
          </Row>
          <Row label="Request timeout (ms)">
            <Input type="number" className="w-32" value={s.timeoutMs} onChange={e => update({ timeoutMs: Number(e.target.value) || 0 })} />
          </Row>
          <Row label="Follow redirects">
            <Toggle checked={s.followRedirects} onChange={v => update({ followRedirects: v })} />
          </Row>
          <Row label="Max redirects">
            <Input type="number" className="w-32" value={s.maxRedirects} onChange={e => update({ maxRedirects: Number(e.target.value) || 0 })} />
          </Row>
          <Row label="Validate TLS certificates">
            <Toggle checked={s.validateCertificates} onChange={v => update({ validateCertificates: v })} />
          </Row>
          <Row label="Responses kept per request">
            <Input
              type="number"
              className="w-32"
              value={s.maxHistoryPerRequest}
              onChange={e => update({ maxHistoryPerRequest: Number(e.target.value) || 1 })}
            />
          </Row>
          <p className="text-[12px] text-muted">Settings are saved immediately and shared with the irs command-line tool.</p>
        </div>
      )}
    </Modal>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4">
      <span>{label}</span>
      {children}
    </div>
  );
}
