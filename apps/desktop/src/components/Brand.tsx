import { Monitor, Moon, Sun } from 'lucide-react';

import { cn } from '../lib/utils';

/** Logic Socket mark: two linked rings on a blue tile. */
export function Logo({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 32 32" className={cn('size-6', className)} aria-hidden="true">
      <defs>
        <linearGradient id="ls-logo" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#8EB0FF" />
          <stop offset="1" stopColor="#2F6BFF" />
        </linearGradient>
      </defs>
      <rect x="2" y="2" width="28" height="28" rx="9" fill="url(#ls-logo)" />
      <circle cx="12.5" cy="16" r="3.6" fill="none" stroke="#fff" strokeWidth="2.4" />
      <circle cx="19.5" cy="16" r="3.6" fill="none" stroke="#fff" strokeWidth="2.4" />
    </svg>
  );
}

const MODES = [
  { id: 'light', label: 'Light', icon: Sun },
  { id: 'dark', label: 'Dark', icon: Moon },
  { id: 'system', label: 'System', icon: Monitor },
] as const;

/** Light / Dark / System segmented switch. */
export function ThemeSwitch({ theme, setTheme, compact }: { theme: string; setTheme: (t: string) => void; compact?: boolean }) {
  return (
    <div role="radiogroup" aria-label="Theme" className="inline-flex items-center gap-0.5 rounded-full border border-app bg-app p-[3px] shadow-sm">
      {MODES.map(m => (
        <button
          key={m.id}
          role="radio"
          aria-checked={theme === m.id}
          title={`${m.label} theme`}
          onClick={() => setTheme(m.id)}
          className={cn(
            'flex h-6 items-center gap-1.5 rounded-full px-2 text-[12px] font-medium transition-colors',
            theme === m.id ? 'bg-accent text-on-accent' : 'text-muted hover:text-app',
          )}
        >
          <m.icon className="size-3.5" />
          {!compact && m.label}
        </button>
      ))}
    </div>
  );
}
