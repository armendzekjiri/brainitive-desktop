// Helpers shared by the native windows (capture, switcher, settings). Bundled pages only: they
// call their own Rust commands and never see a token. Server-provided text is set with
// textContent only.

export const $ = <T extends HTMLElement = HTMLElement>(id: string): T => {
    const el = document.getElementById(id);
    if (!el) throw new Error(`#${id} missing`);
    return el as T;
};

export function errorMessage(e: unknown): string {
    if (e && typeof e === 'object' && 'message' in e) return String((e as { message: unknown }).message);
    return String(e);
}

export function busy(button: HTMLButtonElement, on: boolean): void {
    button.disabled = on;
    button.setAttribute('aria-busy', String(on));
}

/** Runs `fn` whenever the (hidden, reused) window comes to the front again. */
export function onShown(fn: () => void): void {
    window.addEventListener('focus', fn);
    document.addEventListener('visibilitychange', () => {
        if (document.visibilityState === 'visible') fn();
    });
}
