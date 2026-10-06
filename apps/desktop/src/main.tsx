import '@fontsource-variable/manrope';
import '@fontsource-variable/jetbrains-mono';
import './index.css';

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { App } from './App';

async function start() {
  if (import.meta.env.VITE_WEB_BRIDGE) {
    (await import('./lib/webBridge')).installWebBridge();
  }
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

start();
