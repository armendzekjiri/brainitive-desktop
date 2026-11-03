// The main window's top bar: app menus (native menus that open under their buttons), back /
// forward / reload, the tabs, the server and the window controls. Everything goes through the
// bar's own Rust commands (capabilities/topbar.json). This page never sees a token or a URL:
// tabs are labels, titles and icon kinds. Page titles and the server's name are server text and
// are set with textContent only.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { $ } from './native';

interface TabView {
    id: string;
    title: string;
    kind: string;
}

interface View {
    platform: 'macos' | 'windows' | 'linux';
    in_app: boolean;
    server: string | null;
    offline: boolean;
    tabs: TabView[];
    active: string;
    maximized: boolean;
    menus: [string, string][];
}

const SVG_NS = 'http://www.w3.org/2000/svg';

// 16×16 line icons (paths only: constants of this file, never server text)
const ICONS: Record<string, string[]> = {
    home: ['M2.8 7.2 8 3l5.2 4.2', 'M4.2 6.3v6.2h7.6V6.3', 'M6.7 12.5V9.4h2.6v3.1'],
    board: ['M3 3.5h10v9H3z', 'M6.3 3.5v9', 'M9.7 3.5v9'],
    doc: ['M4 2.5h5.2L12 5.3v8.2H4z', 'M9 2.5v3h3', 'M6 8.3h4', 'M6 10.6h4'],
    whiteboard: ['M2.5 3.5h11v7.5h-11z', 'M8 11v2.5', 'M5.5 13.5h5', 'M5 8.5l2-2 1.5 1.5L11 5.5'],
    plan: ['M3 4h5', 'M5 8h6', 'M7.5 12H13', 'M2.5 2.5v11'],
    calendar: ['M3 4h10v9H3z', 'M3 6.8h10', 'M5.8 2.5v2.5', 'M10.2 2.5v2.5'],
    bell: ['M4.3 10.8V7.5a3.7 3.7 0 0 1 7.4 0v3.3l1.1 1.3H3.2z', 'M6.7 13.4a1.5 1.5 0 0 0 2.6 0'],
    check: ['M3 3h10v10H3z', 'm5.5 8 1.8 1.8L10.7 6.4'],
    inbox: ['M2.5 9.2 4.3 3.5h7.4l1.8 5.7v3.8h-11z', 'M2.5 9.2h3l1 1.6h3l1-1.6h3'],
    bookmark: ['M4.5 2.5h7v11L8 10.8l-3.5 2.7z'],
    page: ['M4 2.5h8v11H4z', 'M6 5.5h4', 'M6 8h4', 'M6 10.5h2.5'],
};

function icon(kind: string, cls: string): SVGSVGElement {
    const svg = document.createElementNS(SVG_NS, 'svg');
    svg.setAttribute('viewBox', '0 0 16 16');
    svg.setAttribute('aria-hidden', 'true');
    svg.setAttribute('class', cls);
    for (const d of ICONS[kind] ?? ICONS.page ?? []) {
        const path = document.createElementNS(SVG_NS, 'path');
        path.setAttribute('d', d);
        svg.append(path);
    }
    return svg;
}

function closeIcon(): SVGSVGElement {
    const svg = document.createElementNS(SVG_NS, 'svg');
    svg.setAttribute('viewBox', '0 0 12 12');
    svg.setAttribute('aria-hidden', 'true');
    const path = document.createElementNS(SVG_NS, 'path');
    path.setAttribute('d', 'm3 3 6 6M9 3 3 9');
    svg.append(path);
    return svg;
}

let view: View | null = null;
let platformSet = false;

function report(e: unknown): void {
    // a refused call (window closing, tab gone) has nothing to show here
    console.debug('topbar', e);
}

/** A native menu under `el` (or at the pointer for a tab's menu). */
function openMenu(menu: string, el: HTMLElement | null, at?: { x: number; y: number }, tab?: string): void {
    const rect = el?.getBoundingClientRect();
    const x = at?.x ?? rect?.left ?? 0;
    const y = at?.y ?? (rect ? rect.bottom + 4 : 76);
    el?.setAttribute('aria-expanded', 'true');
    invoke('topbar_menu', { menu, x: Math.max(0, Math.round(x)), y: Math.max(0, Math.round(y)), tab: tab ?? null })
        .catch(report)
        .finally(() => el?.setAttribute('aria-expanded', 'false'));
}

function renderMenus(v: View): void {
    const nav = $('menus');
    if (nav.childElementCount === v.menus.length) return;
    nav.replaceChildren();
    for (const [id, text] of v.menus) {
        const b = document.createElement('button');
        b.type = 'button';
        b.className = 'menu-btn';
        b.textContent = text;
        b.setAttribute('aria-haspopup', 'menu');
        b.setAttribute('aria-expanded', 'false');
        b.addEventListener('click', () => openMenu(id, b));
        // moving across the menu buttons while one is open is handled by the native menu itself
        nav.append(b);
    }
}

let tabsShown = false;

function renderTabs(v: View): void {
    const box = $('tabs');
    box.classList.toggle('single', v.tabs.length < 2);
    const existing = new Map<string, HTMLElement>();
    for (const el of Array.from(box.children) as HTMLElement[]) existing.set(el.dataset.id ?? '', el);
    const order: HTMLElement[] = [];
    for (const t of v.tabs) {
        let el = existing.get(t.id);
        if (!el) {
            el = makeTab(t.id);
            // a tab opened while the bar is up slides open (not the ones of the first render)
            if (tabsShown) el.classList.add('tab-enter');
        }
        existing.delete(t.id);
        const label = el.querySelector('.label') as HTMLElement;
        if (label.textContent !== t.title) label.textContent = t.title;
        el.title = t.title;
        if (el.dataset.kind !== t.kind) {
            el.dataset.kind = t.kind;
            el.querySelector('.kind')?.replaceWith(icon(t.kind, 'kind'));
        }
        const active = t.id === v.active;
        el.setAttribute('aria-selected', String(active));
        el.tabIndex = active ? 0 : -1;
        order.push(el);
    }
    for (const el of existing.values()) el.remove();
    tabsShown = v.tabs.length > 0;
    // keep DOM order = model order (moving nodes only when needed)
    order.forEach((el, i) => {
        if (box.children[i] !== el) box.insertBefore(el, box.children[i] ?? null);
    });
}

function makeTab(id: string): HTMLElement {
    const el = document.createElement('div');
    el.className = 'tab';
    el.dataset.id = id;
    el.setAttribute('role', 'tab');
    const divider = document.createElement('span');
    divider.className = 'divider';
    el.append(divider, icon('page', 'kind'));
    const label = document.createElement('span');
    label.className = 'label';
    el.append(label);
    const x = document.createElement('button');
    x.type = 'button';
    x.className = 'x';
    x.title = 'Close tab (Ctrl+W)';
    x.setAttribute('aria-label', 'Close tab');
    x.tabIndex = -1;
    x.append(closeIcon());
    x.addEventListener('pointerdown', (e) => e.stopPropagation());
    x.addEventListener('click', (e) => {
        e.stopPropagation();
        invoke('topbar_close', { id }).catch(report);
    });
    el.append(x);

    el.addEventListener('pointerdown', (e) => startTabDrag(e, el));
    el.addEventListener('auxclick', (e) => {
        if (e.button === 1) {
            e.preventDefault();
            invoke('topbar_close', { id }).catch(report);
        }
    });
    el.addEventListener('mousedown', (e) => {
        // no autoscroll on middle click
        if (e.button === 1) e.preventDefault();
    });
    el.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        openMenu('tab', null, { x: e.clientX, y: e.clientY }, id);
    });
    el.addEventListener('keydown', (e) => {
        if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
            const sib = (e.key === 'ArrowRight' ? el.nextElementSibling : el.previousElementSibling) as HTMLElement | null;
            if (sib?.dataset.id) {
                sib.focus();
                invoke('topbar_select', { id: sib.dataset.id }).catch(report);
            }
        }
    });
    return el;
}

// --- dragging tabs to reorder ------------------------------------------------------------------

function startTabDrag(e: PointerEvent, el: HTMLElement): void {
    if (e.button !== 0) return;
    const id = el.dataset.id ?? '';
    const box = $('tabs');
    const startX = e.clientX;
    let dragging = false;
    const tabs = (): HTMLElement[] => Array.from(box.children) as HTMLElement[];
    const startIndex = tabs().indexOf(el);

    const move = (ev: PointerEvent): void => {
        const dx = ev.clientX - startX;
        if (!dragging) {
            if (Math.abs(dx) < 6) return;
            dragging = true;
            el.classList.add('dragging');
            el.setPointerCapture(ev.pointerId);
        }
        // swap with a neighbour once the pointer passes its middle
        const list = tabs();
        const i = list.indexOf(el);
        const next = list[i + 1];
        const prev = list[i - 1];
        if (next && ev.clientX > next.getBoundingClientRect().left + next.offsetWidth / 2) {
            box.insertBefore(next, el);
        } else if (prev && ev.clientX < prev.getBoundingClientRect().left + prev.offsetWidth / 2) {
            box.insertBefore(el, prev);
        }
    };
    const up = (): void => {
        window.removeEventListener('pointermove', move);
        window.removeEventListener('pointerup', up);
        el.classList.remove('dragging');
        if (dragging) {
            const to = tabs().indexOf(el);
            if (to !== startIndex) invoke('topbar_move', { id, to }).catch(report);
        } else if (view?.active !== id) {
            invoke('topbar_select', { id }).catch(report);
        }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
}

// --- the rest of the bar -----------------------------------------------------------------------

function render(v: View): void {
    view = v;
    if (!platformSet) {
        document.documentElement.classList.add(v.platform);
        $('controls').hidden = v.platform === 'macos';
        platformSet = true;
    }
    renderMenus(v);
    $('nav').hidden = !v.in_app;
    $('new-tab').hidden = !v.in_app;
    $('tabs').hidden = !v.in_app;
    const title = $('title');
    title.hidden = v.in_app;
    title.textContent = v.server ? `Brainitive · ${v.server}` : 'Brainitive';
    renderTabs(v);

    const server = $('server');
    server.hidden = !v.in_app || !v.server;
    $('server-name').textContent = v.server ?? '';
    $('server-dot').classList.toggle('offline', v.offline);
    server.title = v.offline ? `${v.server ?? ''} — offline · Switch server` : 'Switch server';

    $('max-icon').toggleAttribute('hidden', v.maximized);
    $('restore-icon').toggleAttribute('hidden', !v.maximized);
    $('max').title = v.maximized ? 'Restore' : 'Maximize';
    $('max').setAttribute('aria-label', v.maximized ? 'Restore' : 'Maximize');
    fit();
}

/** Six menus when there is room, one ☰ when the window is narrow. */
function fit(): void {
    document.documentElement.classList.toggle('compact', window.innerWidth < 640);
}

async function refresh(): Promise<void> {
    try {
        render(await invoke<View>('topbar_view'));
    } catch (e) {
        report(e);
    }
}

// dragging the window by the bar's empty parts; double click maximizes (Windows / Linux) or
// zooms (macOS)
document.addEventListener('mousedown', (e) => {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (!target.closest('[data-drag]') || target.closest('button, .tab')) return;
    if (e.detail === 2) {
        invoke('topbar_window', { action: 'maximize' }).catch(report);
    } else {
        invoke('topbar_window', { action: 'drag' }).catch(report);
    }
});

for (const b of Array.from(document.querySelectorAll<HTMLElement>('[data-win]'))) {
    b.addEventListener('click', () => invoke('topbar_window', { action: b.dataset.win }).catch(report));
}
for (const b of Array.from(document.querySelectorAll<HTMLElement>('[data-nav]'))) {
    b.addEventListener('click', () => invoke('topbar_nav', { action: b.dataset.nav }).catch(report));
}
$('new-tab').addEventListener('click', () => invoke('topbar_new_tab').catch(report));
$('server').addEventListener('click', () => openMenu('servers', $('server')));
$('menu-all').addEventListener('click', () => openMenu('all', $('menu-all')));

window.addEventListener('resize', fit);
document.addEventListener('contextmenu', (e) => e.preventDefault());

void listen('topbar://changed', () => void refresh());
void refresh();
