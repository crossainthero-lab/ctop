import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';

// A blank window hides failures; surface any startup/runtime error on screen.
function showFatal(msg: string) {
  const el = document.createElement('pre');
  el.style.cssText = 'position:fixed;inset:0;margin:0;padding:24px;background:#fff;color:#b0402a;font:13px monospace;white-space:pre-wrap;z-index:99999;overflow:auto';
  el.textContent = 'CtoP failed to start\n\n' + msg;
  document.body.appendChild(el);
}
window.addEventListener('error', (e) => e.message.startsWith('ResizeObserver') || showFatal(`${e.message}\n${e.error?.stack ?? ''}`));
window.addEventListener('unhandledrejection', (e) => showFatal(String((e.reason as Error)?.stack ?? e.reason)));

let app: ReturnType<typeof mount> | undefined;
try {
  app = mount(App, { target: document.getElementById('app')! });
} catch (e) {
  showFatal(String((e as Error)?.stack ?? e));
}
export default app;
