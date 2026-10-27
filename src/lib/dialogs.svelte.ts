// Promise-based confirmation dialogs with optional "I have checked…" gates.
export interface ConfirmSpec {
  title: string;
  body?: string;
  checks?: string[];
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  tone?: 'normal' | 'danger';
}
export const dlg = $state<{ spec: (ConfirmSpec & { resolve: (ok: boolean) => void }) | null }>({ spec: null });

export function confirmDialog(spec: ConfirmSpec): Promise<boolean> {
  return new Promise((resolve) => {
    dlg.spec = { ...spec, resolve: (ok) => { dlg.spec = null; resolve(ok); } };
  });
}
