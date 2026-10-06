import { Check, Eye, EyeOff, KeyRound, Lock, LockOpen, Plus, Trash2 } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';

import { CodeEditor } from '../components/editors';
import { Button, CopyButton, IconButton, Input, Modal, Select, Toggle, useDialog, useToast } from '../components/ui';
import { api, type EnvList, type Environment, errorText, type Settings, type VaultStatus } from '../lib/api';
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
    if (sel) setText(JSON.stringify(plainData(sel), null, 2));
    setErr(null);
  }, [sel?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  // the JSON editor holds plain variables; secrets are edited separately and kept as stored
  const merged = (env: Environment, t: string) => {
    const data = JSON.parse(t || '{}');
    if (!data || typeof data !== 'object' || Array.isArray(data)) throw new Error('must be a JSON object');
    const secrets = Object.fromEntries(env.secretKeys.filter(k => k in env.data).map(k => [k, env.data[k]]));
    return { ...secrets, ...data };
  };
  const save = async (env: Environment, t: string) => {
    try {
      const data = merged(env, t);
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
                onBlur={() => api.envUpdate({ ...sel, data: merged(sel, text) }).catch(() => {})}
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
            <SecretsPanel
              env={sel}
              onChanged={async () => {
                const l = await api.envList(workspaceId);
                setList(l);
                const fresh = [l.base, ...l.subs].find(e => e.id === sel.id);
                if (fresh) setText(JSON.stringify(plainData(fresh), null, 2));
              }}
            />
          </div>
        </div>
      )}
    </Modal>
  );
}

function plainData(env: Environment) {
  return Object.fromEntries(Object.entries(env.data).filter(([k]) => !env.secretKeys.includes(k)));
}

function SecretsPanel({ env, onChanged }: { env: Environment; onChanged: () => void }) {
  const toast = useToast();
  const dialog = useDialog();
  const [shown, setShown] = useState<Record<string, string>>({});
  const [name, setName] = useState('');
  const [value, setValue] = useState('');
  const [editing, setEditing] = useState<string | null>(null);
  useEffect(() => {
    setShown({});
    setEditing(null);
  }, [env.id]);
  const run = async (f: () => Promise<unknown>) => {
    try {
      await f();
      onChanged();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  };
  const plainKeys = Object.keys(env.data).filter(k => !env.secretKeys.includes(k));
  return (
    <div className="shrink-0 border-t border-app bg-subtle px-3 py-2.5">
      <div className="mb-1.5 flex items-center gap-1.5 text-[12.5px] font-semibold">
        <Lock className="size-3.5" /> Secrets
        <span className="font-normal text-muted">— encrypted on this computer, never exported or committed. Use like any variable.</span>
      </div>
      {env.secretKeys.map(k => (
        <div key={k} className="flex h-8 items-center gap-2">
          <KeyRound className="size-3.5 text-amber-600" />
          <span className="w-40 truncate font-mono text-[12.5px]">{k}</span>
          {editing === k ? (
            <Input
              autoFocus
              type="password"
              className="h-7 flex-1"
              placeholder="New value"
              onKeyDown={e => {
                if (e.key === 'Escape') setEditing(null);
                if (e.key === 'Enter') {
                  const v = (e.target as HTMLInputElement).value;
                  setEditing(null);
                  run(() => api.envSetVar(env.id, k, v, true));
                }
              }}
            />
          ) : (
            <button className="min-w-0 flex-1 truncate text-left font-mono text-[12.5px] text-muted hover:text-app" title="Click to change" onClick={() => setEditing(k)}>
              {k in shown ? shown[k] || '(empty)' : env.data[k] ? '••••••••••' : '(not set on this computer)'}
            </button>
          )}
          {k in shown && <CopyButton text={shown[k]} />}
          <IconButton
            label={k in shown ? 'Hide' : 'Reveal'}
            onClick={async () => {
              if (k in shown) {
                const { [k]: _, ...rest } = shown;
                setShown(rest);
              } else {
                try {
                  setShown({ ...shown, [k]: await api.envReveal(env.id, k) });
                } catch (e) {
                  toast(errorText(e), 'error');
                }
              }
            }}
          >
            {k in shown ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
          </IconButton>
          <IconButton
            label="Make it a plain variable"
            onClick={async () => {
              if (!(await dialog.confirm(`Store “${k}” unencrypted?`, { message: 'It will be visible in the editor, exports and Git.', confirmLabel: 'Make plain' }))) return;
              run(async () => api.envSetVar(env.id, k, await api.envReveal(env.id, k), false));
            }}
          >
            <LockOpen className="size-3.5" />
          </IconButton>
          <IconButton
            label="Delete"
            onClick={async () => {
              if (!(await dialog.confirm(`Delete secret “${k}”?`, { danger: true, confirmLabel: 'Delete' }))) return;
              const data = { ...env.data };
              delete data[k];
              run(() => api.envUpdate({ ...env, data, secretKeys: env.secretKeys.filter(x => x !== k) }));
            }}
          >
            <Trash2 className="size-3.5" />
          </IconButton>
        </div>
      ))}
      <form
        className="mt-1 flex items-center gap-2"
        onSubmit={e => {
          e.preventDefault();
          if (!name.trim()) return;
          const key = name.trim();
          run(async () => {
            await api.envSetVar(env.id, key, value, true);
            setName('');
            setValue('');
          });
        }}
      >
        <Input className="h-7 w-44 font-mono" list={`plain-${env.id}`} placeholder="name (or an existing variable)" value={name} onChange={e => setName(e.target.value)} />
        <datalist id={`plain-${env.id}`}>
          {plainKeys.map(k => (
            <option key={k} value={k} />
          ))}
        </datalist>
        <Input className="h-7 flex-1" type="password" placeholder="value" value={value} onChange={e => setValue(e.target.value)} />
        <Button size="sm" type="submit" disabled={!name.trim()}>
          <Plus className="size-3.5" /> Add secret
        </Button>
      </form>
    </div>
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
