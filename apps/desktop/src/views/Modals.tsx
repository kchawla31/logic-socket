import { Check, Lock, Plus } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';

import { ThemeSwitch } from '../components/Brand';
import { VarMapEditor } from '../components/VarMapEditor';
import { Button, CopyButton, Input, Modal, Toggle, useDialog, useToast } from '../components/ui';
import { invalidateContextVars } from '../components/VarInput';
import { api, type EnvList, type Environment, errorText, type Settings, type VaultStatus } from '../lib/api';
import { cn } from '../lib/utils';

export function EnvironmentModal({
  open,
  onClose,
  workspaceId,
  onUse,
}: {
  open: boolean;
  onClose: () => void;
  workspaceId: string;
  onUse: (id: string | null) => Promise<void>;
}) {
  const toast = useToast();
  const dialog = useDialog();
  const [list, setList] = useState<EnvList | null>(null);
  const [selId, setSelId] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [gen, setGen] = useState(0);
  const envRef = useRef<Environment | null>(null);

  const reload = () =>
    api.envList(workspaceId).then(l => {
      setList(l);
      setGen(n => n + 1);
    });
  useEffect(() => {
    if (open) reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, workspaceId]);

  const all = useMemo(() => (list ? [list.base, ...list.subs] : []), [list]);
  const sel = all.find(e => e.id === selId) ?? all[0];
  if (sel) envRef.current = sel;
  useEffect(() => {
    setErr(null);
  }, [sel?.id]);

  const commit = (data: Record<string, unknown>, secretKeys: string[]) => {
    const cur = envRef.current;
    if (!cur || !list) return;
    const next = { ...cur, data, secretKeys };
    envRef.current = next;
    setList(list.base.id === next.id ? { ...list, base: next } : { ...list, subs: list.subs.map(s => (s.id === next.id ? next : s)) });
    setErr(null);
    api.envUpdate(next).then(() => invalidateContextVars()).catch(e => setErr(errorText(e)));
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
                onChange={e => {
                  const name = e.target.value;
                  envRef.current = envRef.current && envRef.current.id === sel.id ? { ...envRef.current, name } : { ...sel, name };
                  setList({ ...list, subs: list.subs.map(s => (s.id === sel.id ? { ...s, name } : s)) });
                }}
                onBlur={() => {
                  const cur = envRef.current;
                  if (cur && cur.id !== list.base.id) api.itemRename(cur.id, cur.name).catch(e => setErr(errorText(e)));
                }}
              />
              <div className="flex-1" />
              {sel.id !== list.base.id &&
                (list.activeId === sel.id ? (
                  <Button size="sm" onClick={() => onUse(null).then(reload)}>
                    Deactivate
                  </Button>
                ) : (
                  <Button size="sm" variant="primary" onClick={() => onUse(sel.id).then(reload)}>
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
            <VarMapEditor
              docId={`${sel.id}:${gen}`}
              data={sel.data}
              secretKeys={sel.secretKeys}
              contextId={workspaceId}
              error={err}
              hint={
                sel.id === list.base.id
                  ? 'Base variables are always available. The active sub-environment overrides them. Reference with ${name}; values may reference other variables.'
                  : 'Overrides the base environment while active. Reference with ${name}; values may reference other variables.'
              }
              onChange={commit}
              onReveal={key => api.envReveal(sel.id, key)}
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
            <ThemeSwitch theme={theme} setTheme={setTheme} />
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
          <Row label="Script timeout (ms)">
            <Input type="number" className="w-32" value={s.scriptTimeoutMs} onChange={e => update({ scriptTimeoutMs: Number(e.target.value) || 5000 })} />
          </Row>
          <div className="flex flex-col gap-1.5 border-t border-app pt-3">
            <span>Proxy</span>
            <Input value={s.proxyUrl} placeholder="http://proxy.example.com:3128 (empty: HTTP(S)_PROXY env vars)" onChange={e => update({ proxyUrl: e.target.value })} />
            <Input value={s.noProxy} placeholder="Bypass: localhost,127.0.0.1,.internal" onChange={e => update({ noProxy: e.target.value })} />
          </div>
          <VaultSection />
          <p className="text-[12px] text-muted">Settings are saved immediately and shared with the lsock command-line tool.</p>
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

function VaultSection() {
  const toast = useToast();
  const dialog = useDialog();
  const [st, setSt] = useState<VaultStatus | null>(null);
  const [key, setKey] = useState<string | null>(null);
  const [importing, setImporting] = useState('');
  const reload = () => api.vaultStatus().then(setSt).catch(() => {});
  useEffect(() => {
    reload();
  }, []);
  if (!st) return null;
  return (
    <div className="flex flex-col gap-2 border-t border-app pt-3">
      <div className="flex items-center gap-2">
        <Lock className="size-3.5" />
        <span>Secrets vault</span>
        <span className="flex-1" />
        <span className="text-[12px] text-muted">
          {st.hasKey ? 'key in your keychain' : 'no key yet'} · {st.sealedValues} encrypted value{st.sealedValues === 1 ? '' : 's'}
        </span>
      </div>
      <p className="text-[12px] text-muted">To use your secrets on another computer, copy the recovery key there (Settings → Secrets vault → Import key, or <code className="font-mono">lsock vault import-key</code>).</p>
      <div className="flex flex-wrap gap-2">
        {key ? (
          <>
            <Input readOnly value={key} className="flex-1 font-mono text-[11.5px]" />
            <CopyButton text={key} />
            <Button size="sm" variant="ghost" onClick={() => setKey(null)}>
              Hide
            </Button>
          </>
        ) : (
          <Button
            size="sm"
            onClick={async () => {
              if (await dialog.confirm('Show the vault recovery key?', { message: 'Anyone with this key and your database can read your secrets. Store it in a password manager.', confirmLabel: 'Show key' }))
                api.vaultExportKey().then(k => (setKey(k), reload())).catch(e => toast(errorText(e), 'error'));
            }}
          >
            Show recovery key
          </Button>
        )}
      </div>
      <div className="flex gap-2">
        <Input type="password" placeholder="Paste a recovery key from another computer" value={importing} onChange={e => setImporting(e.target.value)} />
        <Button
          size="sm"
          disabled={!importing.trim()}
          onClick={() =>
            api
              .vaultImportKey(importing.trim())
              .then(() => (setImporting(''), reload(), toast('Vault key installed', 'success')))
              .catch(e => toast(errorText(e), 'error'))
          }
        >
          Import key
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={async () => {
            if (!(await dialog.confirm('Reset the vault?', { message: 'Deletes the key and blanks every secret value. This cannot be undone.', danger: true, confirmLabel: 'Reset' }))) return;
            api.vaultReset().then(n => (reload(), toast(`${n} secret value(s) cleared`, 'info'))).catch(e => toast(errorText(e), 'error'));
          }}
        >
          Reset
        </Button>
      </div>
    </div>
  );
}
