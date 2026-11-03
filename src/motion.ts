// Motion helpers for the bundled pages (see motion.css). Pure presentation: no data, no IPC.

const reduced = (): boolean => window.matchMedia('(prefers-reduced-motion: reduce)').matches;

/** Restarts a CSS animation class on `el` (removing and re-adding it after a reflow). */
export function replay(el: Element, cls: string): void {
    el.classList.remove(cls);
    // reading a layout property forces the browser to apply the removal first
    void (el as HTMLElement).offsetWidth;
    el.classList.add(cls);
}

/** Shows `el` (unhides it) with a short slide-in; nothing when it was already visible. */
export function swapIn(el: HTMLElement): void {
    const wasHidden = el.hidden;
    el.hidden = false;
    if (wasHidden && !reduced()) replay(el, 'm-in');
}

/** Lets the rows of `list` cascade in (call after filling it). */
export function stagger(list: HTMLElement): void {
    if (reduced()) return;
    Array.from(list.children).forEach((child, i) => (child as HTMLElement).style.setProperty('--i', String(i)));
    replay(list, 'm-stagger');
}

let shownOnce = false;

/** The window became visible (first load, or shown again after being hidden): its content rises in. */
function windowShown(): void {
    if (reduced()) return;
    replay(document.documentElement, 'm-show');
    window.setTimeout(() => document.documentElement.classList.remove('m-show'), 400);
}

/** Animates the page in whenever its (reused, hidden) window is shown again. */
export function animateWindow(): void {
    if (shownOnce) return;
    shownOnce = true;
    windowShown();
    document.addEventListener('visibilitychange', () => {
        if (document.visibilityState === 'visible') windowShown();
    });
}
