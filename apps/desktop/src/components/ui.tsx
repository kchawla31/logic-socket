import { Loader2, X } from 'lucide-react';
import {
  type ButtonHTMLAttributes,
  createContext,
  type InputHTMLAttributes,
  type ReactNode,
  type SelectHTMLAttributes,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from 'react';

import { cn } from '../lib/utils';

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger';

export function Button({
  variant = 'secondary',
  size = 'md',
  loading,
  className,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: 'sm' | 'md'; loading?: boolean }) {
  return (
    <button
      {...rest}
      disabled={rest.disabled || loading}
      className={cn(
        'inline-flex items-center justify-center gap-1.5 rounded-md font-medium whitespace-nowrap transition-colors disabled:opacity-50 disabled:cursor-not-allowed',
        size === 'sm' ? 'h-7 px-2.5 text-xs' : 'h-8 px-3 text-[13px]',
        variant === 'primary' && 'bg-accent text-white hover:bg-accent-hover',
        variant === 'secondary' && 'border border-app bg-app hover:bg-muted',
        variant === 'ghost' && 'hover:bg-muted text-muted hover:text-app',
        variant === 'danger' && 'bg-rose-600 text-white hover:bg-rose-700',
        className,
      )}
    >
      {loading && <Loader2 className="size-3.5 animate-spin" />}
      {children}
    </button>
  );
}

export function IconButton({
  label,
  className,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button
      aria-label={label}
      title={label}
      {...rest}
      className={cn(
        'inline-flex size-7 items-center justify-center rounded-md text-muted hover:bg-muted hover:text-app disabled:opacity-40',
        className,
      )}
    >
      {children}
    </button>
  );
}

export function Input({ className, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      {...rest}
      className={cn(
        'h-8 w-full rounded-md border border-app bg-app px-2.5 text-[13px] outline-none placeholder:text-muted focus:border-accent',
        className,
      )}
    />
  );
}

export function Select({ className, children, ...rest }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select
      {...rest}
      className={cn(
        'h-8 rounded-md border border-app bg-app px-2 text-[13px] outline-none focus:border-accent',
        className,
      )}
    >
      {children}
    </select>
  );
}

export function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label?: string }) {
  return (
    <label className="inline-flex cursor-pointer items-center gap-2 select-none">
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={label}
        onClick={() => onChange(!checked)}
        className={cn(
          'relative h-4.5 w-8 rounded-full transition-colors',
          checked ? 'bg-accent' : 'bg-muted border border-app',
        )}
      >
        <span
          className={cn(
            'absolute top-0.5 size-3.5 rounded-full bg-white shadow transition-all',
            checked ? 'left-4' : 'left-0.5',
          )}
        />
      </button>
      {label && <span>{label}</span>}
    </label>
  );
}

export interface TabDef {
  id: string;
  label: ReactNode;
  count?: number;
  dot?: boolean;
}

export function Tabs({
  tabs,
  value,
  onChange,
  className,
  right,
}: {
  tabs: TabDef[];
  value: string;
  onChange: (id: string) => void;
  className?: string;
  right?: ReactNode;
}) {
  return (
    <div role="tablist" className={cn('flex h-9 shrink-0 items-end gap-1 border-b border-app px-2', className)}>
      {tabs.map(t => (
        <button
          key={t.id}
          role="tab"
          aria-selected={value === t.id}
          onClick={() => onChange(t.id)}
          className={cn(
            '-mb-px flex h-8 items-center gap-1.5 border-b-2 px-2.5 text-[13px] transition-colors',
            value === t.id ? 'border-accent text-app font-medium' : 'border-transparent text-muted hover:text-app',
          )}
        >
          {t.label}
          {t.count !== undefined && t.count > 0 && (
            <span className="rounded-full bg-muted px-1.5 text-[11px] leading-4 text-muted">{t.count}</span>
          )}
          {t.dot && <span className="size-1.5 rounded-full bg-accent" />}
        </button>
      ))}
      <div className="flex-1" />
      {right && <div className="flex h-8 items-center gap-1">{right}</div>}
    </div>
  );
}

export function Badge({ children, tone = 'neutral', title }: { children: ReactNode; tone?: string; title?: string }) {
  const tones: Record<string, string> = {
    neutral: 'bg-muted text-muted',
    green: 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-400',
    red: 'bg-rose-500/15 text-rose-700 dark:text-rose-400',
    amber: 'bg-amber-500/15 text-amber-700 dark:text-amber-400',
    blue: 'bg-sky-500/15 text-sky-700 dark:text-sky-400',
    violet: 'bg-accent-soft text-accent',
  };
  return (
    <span title={title} className={cn('inline-flex h-5 items-center rounded px-1.5 text-[11px] font-medium whitespace-nowrap', tones[tone])}>
      {children}
    </span>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="rounded border border-app bg-subtle px-1 font-mono text-[10.5px] leading-4 text-muted">{children}</kbd>
  );
}

export function Empty({ icon, title, children }: { icon?: ReactNode; title: string; children?: ReactNode }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 p-8 text-center text-muted">
      {icon && <div className="mb-1 opacity-60">{icon}</div>}
      <div className="text-[14px] font-medium text-app">{title}</div>
      {children && <div className="max-w-sm text-[12.5px] leading-relaxed">{children}</div>}
    </div>
  );
}

export function Modal({
  open,
  onClose,
  title,
  children,
  width = 'max-w-2xl',
  footer,
}: {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  children: ReactNode;
  width?: string;
  footer?: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) d.showModal();
    if (!open && d.open) d.close();
  }, [open]);
  return (
    <dialog
      ref={ref}
      onClose={onClose}
      onClick={e => e.target === ref.current && onClose()}
      className={cn(
        'm-auto w-[92vw] rounded-xl border border-app bg-app p-0 text-app shadow-2xl backdrop:bg-black/40 backdrop:backdrop-blur-[2px]',
        width,
      )}
    >
      {open && (
        <div className="flex max-h-[85vh] flex-col">
          <div className="flex h-11 shrink-0 items-center justify-between border-b border-app px-4">
            <div className="text-[14px] font-semibold">{title}</div>
            <IconButton label="Close" onClick={onClose}>
              <X className="size-4" />
            </IconButton>
          </div>
          <div className="min-h-0 flex-1 overflow-auto">{children}</div>
          {footer && <div className="flex shrink-0 justify-end gap-2 border-t border-app px-4 py-2.5">{footer}</div>}
        </div>
      )}
    </dialog>
  );
}

// ---- toasts

interface Toast {
  id: number;
  text: string;
  tone: 'error' | 'info' | 'success';
}

const ToastCtx = createContext<(text: string, tone?: Toast['tone']) => void>(() => {});

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const push = useCallback((text: string, tone: Toast['tone'] = 'info') => {
    const id = Date.now() + Math.random();
    setToasts(t => [...t.slice(-3), { id, text, tone }]);
    setTimeout(() => setToasts(t => t.filter(x => x.id !== id)), tone === 'error' ? 7000 : 3000);
  }, []);
  return (
    <ToastCtx.Provider value={push}>
      {children}
      <div className="pointer-events-none fixed right-4 bottom-4 z-50 flex flex-col gap-2" aria-live="polite">
        {toasts.map(t => (
          <div
            key={t.id}
            className={cn(
              'pointer-events-auto max-w-md rounded-lg border px-3.5 py-2.5 text-[13px] shadow-lg',
              t.tone === 'error' && 'border-rose-500/40 bg-rose-50 text-rose-800 dark:bg-rose-950 dark:text-rose-200',
              t.tone === 'success' && 'border-emerald-500/40 bg-emerald-50 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-200',
              t.tone === 'info' && 'border-app bg-app',
            )}
          >
            {t.text}
          </div>
        ))}
      </div>
    </ToastCtx.Provider>
  );
}

export const useToast = () => useContext(ToastCtx);

// ---- resizable split

export function Split({
  direction,
  initial,
  min = 200,
  storageKey,
  children,
}: {
  direction: 'row' | 'col';
  initial: number;
  min?: number;
  storageKey: string;
  children: [ReactNode, ReactNode];
}) {
  const [size, setSize] = useState<number>(() => {
    try {
      return Number(localStorage.getItem(storageKey)) || initial;
    } catch {
      return initial;
    }
  });
  const box = useRef<HTMLDivElement>(null);
  const onDown = (e: React.PointerEvent) => {
    e.preventDefault();
    const rect = box.current!.getBoundingClientRect();
    const move = (ev: PointerEvent) => {
      const total = direction === 'row' ? rect.width : rect.height;
      const pos = direction === 'row' ? ev.clientX - rect.left : ev.clientY - rect.top;
      const next = Math.max(min, Math.min(total - min, pos));
      setSize(next);
      try {
        localStorage.setItem(storageKey, String(Math.round(next)));
      } catch {
        /* ignore */
      }
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };
  return (
    <div ref={box} className={cn('flex min-h-0 min-w-0 flex-1', direction === 'row' ? 'flex-row' : 'flex-col')}>
      <div style={{ flexBasis: size }} className="flex min-h-0 min-w-0 shrink-0 flex-col">
        {children[0]}
      </div>
      <div
        role="separator"
        aria-orientation={direction === 'row' ? 'vertical' : 'horizontal'}
        onPointerDown={onDown}
        className={cn(
          'shrink-0 bg-[var(--border)] transition-colors hover:bg-accent',
          direction === 'row' ? 'w-px cursor-col-resize hover:w-0.5' : 'h-px cursor-row-resize hover:h-0.5',
        )}
      />
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">{children[1]}</div>
    </div>
  );
}

export function CopyButton({ text, label = 'Copy' }: { text: string; label?: string }) {
  const [done, setDone] = useState(false);
  return (
    <Button
      size="sm"
      variant="ghost"
      onClick={() => {
        navigator.clipboard?.writeText(text).then(() => {
          setDone(true);
          setTimeout(() => setDone(false), 1200);
        });
      }}
    >
      {done ? 'Copied' : label}
    </Button>
  );
}

// ---- in-app confirm / prompt (native dialogs are unreliable in WKWebView)

interface DialogReq {
  kind: 'confirm' | 'prompt';
  title: string;
  message?: string;
  initial?: string;
  danger?: boolean;
  confirmLabel?: string;
  resolve: (v: string | boolean | null) => void;
}

const DialogCtx = createContext<{
  confirm: (title: string, opts?: { message?: string; danger?: boolean; confirmLabel?: string }) => Promise<boolean>;
  prompt: (title: string, initial?: string) => Promise<string | null>;
}>({ confirm: async () => false, prompt: async () => null });

export function DialogProvider({ children }: { children: ReactNode }) {
  const [req, setReq] = useState<DialogReq | null>(null);
  const [value, setValue] = useState('');
  const confirm = useCallback(
    (title: string, opts?: { message?: string; danger?: boolean; confirmLabel?: string }) =>
      new Promise<boolean>(resolve => setReq({ kind: 'confirm', title, ...opts, resolve: v => resolve(!!v) })),
    [],
  );
  const prompt = useCallback(
    (title: string, initial = '') =>
      new Promise<string | null>(resolve => {
        setValue(initial);
        setReq({ kind: 'prompt', title, initial, resolve: v => resolve(typeof v === 'string' ? v : null) });
      }),
    [],
  );
  const finish = (v: string | boolean | null) => {
    req?.resolve(v);
    setReq(null);
  };
  return (
    <DialogCtx.Provider value={{ confirm, prompt }}>
      {children}
      <Modal
        open={!!req}
        onClose={() => finish(null)}
        title={req?.title ?? ''}
        width="max-w-md"
        footer={
          <>
            <Button onClick={() => finish(null)}>Cancel</Button>
            <Button
              variant={req?.danger ? 'danger' : 'primary'}
              onClick={() => finish(req?.kind === 'prompt' ? value.trim() || null : true)}
            >
              {req?.confirmLabel ?? (req?.kind === 'prompt' ? 'Create' : 'OK')}
            </Button>
          </>
        }
      >
        <div className="p-4">
          {req?.message && <p className="mb-3 leading-relaxed">{req.message}</p>}
          {req?.kind === 'prompt' && (
            <Input
              autoFocus
              value={value}
              onChange={e => setValue(e.target.value)}
              onKeyDown={e => e.key === 'Enter' && finish(value.trim() || null)}
            />
          )}
        </div>
      </Modal>
    </DialogCtx.Provider>
  );
}

export const useDialog = () => useContext(DialogCtx);
