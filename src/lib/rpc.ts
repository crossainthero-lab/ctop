// One call() for both front doors: the desktop window (Tauri command) and the browser (local HTTP service).
// The backend command set is identical; see src-tauri/src/writer.rs `call`.

const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
const TOKEN_KEY = 'ctop-token';

function token(): string {
  try {
    const u = new URL(location.href);
    const t = u.searchParams.get('token');
    if (t) {
      localStorage.setItem(TOKEN_KEY, t);
      u.searchParams.delete('token');
      history.replaceState(null, '', u.pathname + u.search + u.hash); // keep the secret out of the address bar / history
    }
    // plain http://localhost:7795: the service puts the token in the page itself (loopback only)
    const meta = document.querySelector('meta[name="ctop-token"]')?.getAttribute('content');
    if (meta) return meta;
    return localStorage.getItem(TOKEN_KEY) ?? '';
  } catch {
    return document.querySelector('meta[name="ctop-token"]')?.getAttribute('content') ?? '';
  }
}

export const transport: 'desktop' | 'browser' = inTauri ? 'desktop' : 'browser';
const tok = token();

export async function call<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (inTauri) {
    const { invoke } = await import('@tauri-apps/api/core');
    return (await invoke<T>('call', { cmd, args: args ?? null })) as T;
  }
  if (!tok) throw new Error('No access token. Open http://localhost:7795 on the computer that runs CtoP.');
  let res: Response;
  try {
    res = await fetch('/api/call', { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-CtoP-Token': tok }, body: JSON.stringify({ cmd, args: args ?? null }) });
  } catch (e) {
    throw new Error(`CtoP service not reachable (${e instanceof Error ? e.message : e})`);
  }
  if (res.status === 401) throw new Error('Access token rejected. Reload the page (CtoP may have been reinstalled).');
  const j = (await res.json()) as { ok: boolean; result?: T; error?: string };
  if (!j.ok) throw new Error(j.error ?? 'unknown error');
  return j.result as T;
}
