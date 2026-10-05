import { AlertTriangle, ArrowDown, ArrowUp, FolderOpen, GitBranch, GitCommitHorizontal, Link2, Lock, Plus, RefreshCw, RotateCcw, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';

import { Badge, Button, Empty, IconButton, Input, Modal, Select, Tabs, useDialog, useToast } from '../components/ui';
import { api, errorText, type GitCommit, type GitRepo, type GitStatus, type GitSyncResult } from '../lib/api';
import { cn } from '../lib/utils';

const STATUS_TONE: Record<string, string> = { modified: 'amber', added: 'green', untracked: 'green', deleted: 'red', conflict: 'red', renamed: 'blue' };

function DiffView({ text }: { text: string }) {
  if (!text.trim()) return <Empty title="No differences" />;
  return (
    <pre className="min-h-0 flex-1 overflow-auto bg-subtle p-2 font-mono text-[11.5px] leading-[1.45]">
      {text.split('\n').map((l, i) => (
        <div
          key={i}
          className={cn(
            'px-1 whitespace-pre-wrap',
            l.startsWith('+') && !l.startsWith('+++') && 'bg-emerald-500/12 text-emerald-800 dark:text-emerald-300',
            l.startsWith('-') && !l.startsWith('---') && 'bg-rose-500/12 text-rose-800 dark:text-rose-300',
            l.startsWith('@@') && 'text-sky-700 dark:text-sky-400',
            (l.startsWith('diff ') || l.startsWith('index ') || l.startsWith('+++') || l.startsWith('---')) && 'text-muted',
          )}
        >
          {l || ' '}
        </div>
      ))}
    </pre>
  );
}

function Connect({ workspaceName, onConnected }: { workspaceName: string; onConnected: (r: GitRepo, s: GitSyncResult) => void }) {
  const toast = useToast();
  const [url, setUrl] = useState('');
  const [cloneDir, setCloneDir] = useState('');
  const [token, setToken] = useState('');
  const [localDir, setLocalDir] = useState('');
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    api.gitDefaultDir(workspaceName).then(setLocalDir);
  }, [workspaceName]);
  useEffect(() => {
    const name = url.trim().split('/').pop()?.replace(/\.git$/, '') || 'repo';
    api.gitDefaultDir(name).then(setCloneDir);
  }, [url]);

  const run = async (kind: string, f: () => Promise<{ repo: GitRepo; sync: GitSyncResult }>) => {
    setBusy(kind);
    try {
      const r = await f();
      onConnected(r.repo, r.sync);
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="grid gap-4 p-5 md:grid-cols-2">
      <div className="flex flex-col gap-2 rounded-lg border border-app p-4">
        <div className="font-semibold">Clone a repository</div>
        <p className="text-[12px] text-muted">Every workspace file in it (Insomnia v5 YAML) is imported. Works with repositories Insomnia syncs too.</p>
        <Input value={url} onChange={e => setUrl(e.target.value)} placeholder="https://github.com/acme/api-collections.git or git@…" />
        <Input value={cloneDir} onChange={e => setCloneDir(e.target.value)} placeholder="Local folder" />
        <Input type="password" value={token} onChange={e => setToken(e.target.value)} placeholder="Access token (optional — SSH keys and credential helpers also work)" />
        <Button variant="primary" disabled={!url.trim()} loading={busy === 'clone'} onClick={() => run('clone', () => api.gitClone(url.trim(), cloneDir, token || null))}>
          Clone
        </Button>
      </div>
      <div className="flex flex-col gap-2 rounded-lg border border-app p-4">
        <div className="font-semibold">Use a folder on this computer</div>
        <p className="text-[12px] text-muted">An existing Git repository, or a new folder (we run git init). Add a remote later to share it.</p>
        <Input value={localDir} onChange={e => setLocalDir(e.target.value)} placeholder="/Users/you/Documents/insomnia-rs/my-api" />
        <div className="flex-1" />
        <Button variant="primary" disabled={!localDir.trim()} loading={busy === 'open'} onClick={() => run('open', () => api.gitOpen(localDir.trim(), null))}>
          Open or create
        </Button>
      </div>
      <p className="flex items-start gap-1.5 text-[12px] text-muted md:col-span-2">
        <Lock className="mt-0.5 size-3.5 shrink-0" /> Secret values, private environments, cookies and response history never leave this computer. Teammates see secret names and fill in their own values.
      </p>
    </div>
  );
}

export function GitModal({
  open,
  onClose,
  workspaceId,
  workspaceName,
  onOpenWorkspace,
}: {
  open: boolean;
  onClose: () => void;
  workspaceId: string;
  workspaceName: string;
  onOpenWorkspace: (id: string) => void;
}) {
  const toast = useToast();
  const dialog = useDialog();
  const [repos, setRepos] = useState<GitRepo[] | null>(null);
  const [selId, setSelId] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [tab, setTab] = useState('changes');
  const [diffPath, setDiffPath] = useState<string | null>(null);
  const [diff, setDiff] = useState('');
  const [message, setMessage] = useState('');
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [log, setLog] = useState<GitCommit[]>([]);
  const [branches, setBranches] = useState<{ current: string; all: string[] }>({ current: '', all: [] });
  const [conflicts, setConflicts] = useState<string[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [token, setToken] = useState('');

  const repo = useMemo(() => repos?.find(r => r.id === selId) ?? null, [repos, selId]);
  const linked = repo?.workspaces.some(w => w.workspaceId === workspaceId);

  const loadRepos = useCallback(async () => {
    const list = await api.gitRepoList();
    setRepos(list);
    setSelId(cur => cur && list.some(r => r.id === cur) ? cur : (list.find(r => r.workspaces.some(w => w.workspaceId === workspaceId)) ?? list[0])?.id ?? null);
  }, [workspaceId]);

  const refresh = useCallback(async () => {
    if (!repo) return;
    try {
      const [st, b, l] = await Promise.all([api.gitStatus(repo.id), api.gitBranches(repo.id), api.gitLog(repo.id, 50)]);
      setStatus(st);
      setBranches(b);
      setLog(l);
      setConflicts(st.changes.filter(c => c.status === 'conflict').map(c => c.path));
      setPicked(new Set(st.changes.filter(c => c.status !== 'conflict').map(c => c.path)));
      setDiffPath(p => (p && st.changes.some(c => c.path === p) ? p : (st.changes[0]?.path ?? null)));
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }, [repo, toast]);

  useEffect(() => {
    if (open) {
      setAdding(false);
      loadRepos().catch(e => toast(errorText(e), 'error'));
    }
  }, [open, loadRepos, toast]);
  useEffect(() => {
    if (open && repo) refresh();
  }, [open, repo?.id]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (!repo || !diffPath) return setDiff('');
    api.gitDiff(repo.id, diffPath).then(setDiff).catch(e => setDiff(errorText(e)));
  }, [repo, diffPath, status]);

  const act = async (kind: string, f: () => Promise<unknown>, ok?: string) => {
    setBusy(kind);
    try {
      const r = await f();
      const sync = r as Partial<GitSyncResult> | undefined;
      if (sync && Array.isArray(sync.conflicts)) {
        setConflicts(sync.conflicts);
        if (sync.conflicts.length) toast(`Merge conflicts in ${sync.conflicts.join(', ')} — choose a version below`, 'error');
        for (const w of sync.warnings ?? []) toast(w, 'info');
      }
      if (ok && !(sync && sync.conflicts?.length)) toast(ok, 'success');
      await loadRepos();
      await refresh();
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      setBusy(null);
    }
  };

  const showConnect = adding || (repos !== null && repos.length === 0);

  return (
    <Modal open={open} onClose={onClose} title={<span className="flex items-center gap-2"><GitBranch className="size-4" /> Git sync</span>} width="max-w-5xl">
      {repos === null ? null : showConnect ? (
        <>
          {repos.length > 0 && (
            <div className="px-5 pt-3">
              <Button size="sm" variant="ghost" onClick={() => setAdding(false)}>
                ← Back
              </Button>
            </div>
          )}
          <Connect
            workspaceName={workspaceName}
            onConnected={async (r, s) => {
              setAdding(false);
              await loadRepos();
              setSelId(r.id);
              if (s.workspaces.length) toast(`Loaded ${s.workspaces.join(', ')}`, 'success');
              for (const w of s.warnings) toast(w, 'info');
            }}
          />
        </>
      ) : (
        <div className="flex h-[70vh]">
          {/* repositories */}
          <div className="flex w-52 shrink-0 flex-col border-r border-app bg-subtle p-2">
            {repos.map(r => (
              <button key={r.id} onClick={() => setSelId(r.id)} className={cn('mb-0.5 flex flex-col rounded-md px-2.5 py-1.5 text-left', r.id === selId ? 'bg-accent-soft' : 'hover:bg-muted')}>
                <span className="truncate font-medium">{r.name}</span>
                <span className="truncate text-[11px] text-muted">{r.workspaces.map(w => w.name).join(', ') || 'no workspaces yet'}</span>
              </button>
            ))}
            <Button size="sm" variant="ghost" className="mt-1 justify-start" onClick={() => setAdding(true)}>
              <Plus className="size-3.5" /> Add repository
            </Button>
          </div>

          {repo && (
            <div className="flex min-w-0 flex-1 flex-col">
              {/* header */}
              <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-app px-3 py-2">
                <GitBranch className="size-4 text-muted" />
                <Select
                  aria-label="Branch"
                  className="h-7 text-[12.5px]"
                  value={branches.current}
                  onChange={async e => {
                    const v = e.target.value;
                    if (v === '__new') {
                      const name = await dialog.prompt('New branch', '');
                      if (name) act('checkout', () => api.gitCheckout(repo.id, name, true), `Created ${name}`);
                    } else act('checkout', () => api.gitCheckout(repo.id, v, false), `Switched to ${v}`);
                  }}
                >
                  {branches.all.map(b => (
                    <option key={b} value={b}>
                      {b}
                    </option>
                  ))}
                  <option value="__new">+ New branch…</option>
                </Select>
                {status?.upstream && <span className="text-[12px] text-muted">→ {status.upstream}</span>}
                {!!status?.behind && <Badge tone="amber">{status.behind} to pull</Badge>}
                {!!status?.ahead && <Badge tone="blue">{status.ahead} to push</Badge>}
                <div className="flex-1" />
                <IconButton label="Refresh" onClick={() => act('refresh', async () => {})}>
                  <RefreshCw className={cn('size-3.5', busy === 'refresh' && 'animate-spin')} />
                </IconButton>
                <Button size="sm" loading={busy === 'pull'} onClick={() => act('pull', () => api.gitPull(repo.id), 'Pulled')} disabled={!status?.hasRemote}>
                  <ArrowDown className="size-3.5" /> Pull
                </Button>
                <Button size="sm" loading={busy === 'push'} onClick={() => act('push', () => api.gitPush(repo.id), 'Pushed')} disabled={!status?.hasRemote}>
                  <ArrowUp className="size-3.5" /> Push
                </Button>
              </div>

              {!linked && (
                <div className="flex shrink-0 items-center gap-2 border-b border-app bg-accent-soft px-3 py-2 text-[12.5px]">
                  <Link2 className="size-3.5" /> “{workspaceName}” isn’t synced yet.
                  <Button size="sm" variant="primary" loading={busy === 'link'} onClick={() => act('link', () => api.gitLink(repo.id, workspaceId), `Syncing ${workspaceName}`)}>
                    Sync it through {repo.name}
                  </Button>
                </div>
              )}
              {!status?.hasRemote && (
                <div className="shrink-0 border-b border-app px-3 py-1.5 text-[12px] text-muted">No remote yet — add one in Settings to pull and push.</div>
              )}
              {conflicts.length > 0 && (
                <div className="flex shrink-0 flex-col gap-1.5 border-b border-rose-500/30 bg-rose-500/8 px-3 py-2 text-[12.5px]">
                  <div className="flex items-center gap-1.5 font-medium text-rose-700 dark:text-rose-300">
                    <AlertTriangle className="size-3.5" /> Both you and the remote changed these files. Pick the version to keep:
                  </div>
                  {conflicts.map(p => (
                    <div key={p} className="flex items-center gap-2">
                      <span className="font-mono text-[12px]">{p}</span>
                      <span className="flex-1" />
                      <Button size="sm" onClick={() => act('resolve', () => api.gitResolve(repo.id, p, 'ours'), 'Kept your version')}>
                        Keep mine
                      </Button>
                      <Button size="sm" onClick={() => act('resolve', () => api.gitResolve(repo.id, p, 'theirs'), 'Took the remote version')}>
                        Take theirs
                      </Button>
                    </div>
                  ))}
                  <div>
                    <Button size="sm" variant="ghost" onClick={() => act('abort', () => api.gitAbortMerge(repo.id), 'Merge aborted')}>
                      Abort the pull
                    </Button>
                  </div>
                </div>
              )}

              <Tabs
                tabs={[
                  { id: 'changes', label: 'Changes', count: status?.changes.length },
                  { id: 'history', label: 'History' },
                  { id: 'settings', label: 'Settings' },
                ]}
                value={tab}
                onChange={setTab}
              />

              {tab === 'changes' && (
                <div className="flex min-h-0 flex-1">
                  <div className="flex w-72 shrink-0 flex-col border-r border-app">
                    <div className="min-h-0 flex-1 overflow-auto p-1.5">
                      {!status?.changes.length && <Empty title="Nothing to commit" >Edits to synced workspaces show up here.</Empty>}
                      {status?.changes.map(c => (
                        <div
                          key={c.path}
                          onClick={() => setDiffPath(c.path)}
                          className={cn('group flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5', diffPath === c.path ? 'bg-accent-soft' : 'hover:bg-muted')}
                        >
                          <input
                            type="checkbox"
                            aria-label={`Include ${c.path}`}
                            checked={picked.has(c.path)}
                            disabled={c.status === 'conflict'}
                            onClick={e => e.stopPropagation()}
                            onChange={e => {
                              const next = new Set(picked);
                              if (e.target.checked) next.add(c.path);
                              else next.delete(c.path);
                              setPicked(next);
                            }}
                          />
                          <span className="min-w-0 flex-1">
                            <span className="block truncate text-[12.5px] font-medium">{c.workspace ?? c.path}</span>
                            {c.workspace && <span className="block truncate font-mono text-[10.5px] text-muted">{c.path}</span>}
                          </span>
                          <Badge tone={STATUS_TONE[c.status]}>{c.status}</Badge>
                          {c.status === 'modified' && (
                            <IconButton
                              label="Discard changes"
                              className="invisible group-hover:visible"
                              onClick={async e => {
                                e.stopPropagation();
                                if (await dialog.confirm(`Discard your changes to ${c.workspace ?? c.path}?`, { message: 'The workspace goes back to the last commit.', danger: true, confirmLabel: 'Discard' }))
                                  act('discard', () => api.gitDiscard(repo.id, c.path), 'Changes discarded');
                              }}
                            >
                              <RotateCcw className="size-3.5" />
                            </IconButton>
                          )}
                        </div>
                      ))}
                    </div>
                    <div className="flex shrink-0 flex-col gap-2 border-t border-app p-2">
                      <Input value={message} onChange={e => setMessage(e.target.value)} placeholder="Commit message" onKeyDown={e => e.key === 'Enter' && (e.metaKey || e.ctrlKey) && message.trim() && picked.size && document.getElementById('git-commit')?.click()} />
                      <Button
                        id="git-commit"
                        variant="primary"
                        disabled={!message.trim() || !picked.size}
                        loading={busy === 'commit'}
                        onClick={() =>
                          act('commit', async () => {
                            const hash = await api.gitCommit(repo.id, message.trim(), [...picked]);
                            setMessage('');
                            return hash;
                          }, 'Committed')
                        }
                      >
                        <GitCommitHorizontal className="size-3.5" /> Commit {picked.size || ''} file{picked.size === 1 ? '' : 's'}
                      </Button>
                    </div>
                  </div>
                  <div className="flex min-w-0 flex-1 flex-col">{diffPath ? <DiffView text={diff} /> : <Empty title="Select a file to see its changes" />}</div>
                </div>
              )}

              {tab === 'history' && (
                <div className="min-h-0 flex-1 overflow-auto p-2">
                  {!log.length && <Empty title="No commits yet" />}
                  {log.map(c => (
                    <div key={c.hash} className="flex items-baseline gap-3 rounded-md px-2 py-1.5 hover:bg-muted">
                      <span className="font-mono text-[11.5px] text-amber-700 dark:text-amber-400">{c.hash}</span>
                      <span className="min-w-0 flex-1 truncate">{c.message}</span>
                      <span className="text-[12px] text-muted">{c.author}</span>
                      <span className="w-36 text-right text-[12px] text-muted">{new Date(c.timeMs).toLocaleString()}</span>
                    </div>
                  ))}
                </div>
              )}

              {tab === 'settings' && (
                <RepoSettings
                  repo={repo}
                  hasToken={!!status?.hasToken}
                  token={token}
                  setToken={setToken}
                  busy={busy}
                  act={act}
                  onOpenWorkspace={id => {
                    onOpenWorkspace(id);
                    onClose();
                  }}
                  onRemoved={async () => {
                    setSelId(null);
                    await loadRepos();
                  }}
                />
              )}
            </div>
          )}
        </div>
      )}
    </Modal>
  );
}

function RepoSettings({
  repo,
  hasToken,
  token,
  setToken,
  busy,
  act,
  onOpenWorkspace,
  onRemoved,
}: {
  repo: GitRepo;
  hasToken: boolean;
  token: string;
  setToken: (t: string) => void;
  busy: string | null;
  act: (kind: string, f: () => Promise<unknown>, ok?: string) => Promise<void>;
  onOpenWorkspace: (id: string) => void;
  onRemoved: () => void;
}) {
  const dialog = useDialog();
  const [draft, setDraft] = useState(repo);
  useEffect(() => setDraft(repo), [repo]);
  const dirty = draft.remoteUrl !== repo.remoteUrl || draft.authorName !== repo.authorName || draft.authorEmail !== repo.authorEmail || draft.name !== repo.name;
  return (
    <div className="min-h-0 flex-1 overflow-auto p-4">
      <div className="grid max-w-xl grid-cols-[9rem_1fr] items-center gap-x-3 gap-y-2">
        <span>Name</span>
        <Input value={draft.name} onChange={e => setDraft({ ...draft, name: e.target.value })} />
        <span>Folder</span>
        <div className="flex items-center gap-1">
          <span className="min-w-0 flex-1 truncate font-mono text-[12px] text-muted">{repo.path}</span>
          <IconButton label="Show in Finder" onClick={() => api.revealPath(repo.path)}>
            <FolderOpen className="size-3.5" />
          </IconButton>
        </div>
        <span>Remote URL</span>
        <Input value={draft.remoteUrl} placeholder="https://github.com/acme/api.git" onChange={e => setDraft({ ...draft, remoteUrl: e.target.value })} />
        <span>Author name</span>
        <Input value={draft.authorName} placeholder="from your git config" onChange={e => setDraft({ ...draft, authorName: e.target.value })} />
        <span>Author email</span>
        <Input value={draft.authorEmail} placeholder="from your git config" onChange={e => setDraft({ ...draft, authorEmail: e.target.value })} />
        <span />
        <div>
          <Button size="sm" variant="primary" disabled={!dirty} loading={busy === 'save'} onClick={() => act('save', () => api.gitRepoUpdate(draft), 'Saved')}>
            Save
          </Button>
        </div>
        <span>Access token</span>
        <div className="flex gap-2">
          <Input type="password" value={token} placeholder={hasToken ? '•••••••• stored in the keychain' : 'Optional, for HTTPS remotes'} onChange={e => setToken(e.target.value)} />
          <Button size="sm" disabled={!token} onClick={() => act('token', async () => (await api.gitSetToken(repo.id, token), setToken('')), 'Token saved to the keychain')}>
            Save
          </Button>
          {hasToken && (
            <Button size="sm" variant="ghost" onClick={() => act('token', () => api.gitSetToken(repo.id, null), 'Token removed')}>
              Clear
            </Button>
          )}
        </div>
      </div>

      <div className="mt-6 mb-1.5 font-semibold">Synced workspaces</div>
      {!repo.workspaces.length && <p className="text-[12.5px] text-muted">None yet.</p>}
      {repo.workspaces.map(w => (
        <div key={w.workspaceId} className="flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-muted">
          <button className="font-medium hover:underline" onClick={() => onOpenWorkspace(w.workspaceId)}>
            {w.name}
          </button>
          <span className="font-mono text-[11.5px] text-muted">{w.path}</span>
          <span className="flex-1" />
          <Button
            size="sm"
            variant="ghost"
            onClick={async () => {
              if (await dialog.confirm(`Stop syncing “${w.name}”?`, { message: 'The workspace stays here; its file stays in the repository until you commit its removal.', confirmLabel: 'Stop syncing' }))
                act('unlink', () => api.gitUnlink(repo.id, w.workspaceId, false), 'No longer synced');
            }}
          >
            Stop syncing
          </Button>
        </div>
      ))}

      <div className="mt-6 border-t border-app pt-4">
        <Button
          size="sm"
          variant="ghost"
          onClick={async () => {
            if (await dialog.confirm(`Forget “${repo.name}”?`, { message: 'The folder on disk and your workspaces are kept.', danger: true, confirmLabel: 'Forget' })) {
              await act('remove', () => api.gitRemove(repo.id), 'Repository removed');
              onRemoved();
            }
          }}
        >
          <Trash2 className="size-3.5" /> Forget this repository
        </Button>
      </div>
    </div>
  );
}
