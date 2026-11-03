// The quick switcher (BRAI-401): a second global shortcut (⌥⌘K / Ctrl+Alt+K). Searches the
// server through Rust (`switcher_search` → GET /api/v1/search with the app token) and acts on a
// result: open it in the app window, copy a snippet, start the timer on an issue. Server text is
// rendered with textContent only.

import { invoke } from '@tauri-apps/api/core';
import { $, errorMessage, onShown } from './native';
import { animateWindow, stagger } from './motion';

// the window's content rises in each time it is shown (motion.css)
animateWindow();

interface Hit {
    id: string;
    kind: string;
    label: string;
    title: string;
    key: string | null;
    context: string | null;
    url: string | null;
    source_id: number;
    can_copy: boolean;
    can_time: boolean;
}

interface Place {
    kind: string;
    title: string;
    subtitle: string | null;
    url: string;
}

interface SearchView {
    results: Hit[];
    places: Place[];
    total: number;
}

interface Context {
    server: string | null;
    signed_in: boolean;
    platform: 'macos' | 'windows' | 'linux';
}

type Row = { hit?: Hit; place?: Place };

const q = $<HTMLInputElement>('q');
const list = $('results');
const status = $('status');
let rows: Row[] = [];
let selected = 0;
let filter = '';
let seq = 0;
let timer: number | undefined;
let ready = false;

function setStatus(message: string, kind: 'ok' | 'error' | 'muted' = 'muted'): void {
    status.textContent = message;
    status.className = `status ${kind}`;
}

function empty(message: string): void {
    rows = [];
    const li = document.createElement('li');
    li.className = 'empty';
    li.textContent = message;
    list.replaceChildren(li);
}

/** The rows that were last animated in: a new result set cascades in, a selection change doesn't. */
let shownRows: Row[] | null = null;

function render(): void {
    const fresh = shownRows !== rows;
    shownRows = rows;
    list.replaceChildren();
    rows.forEach((row, i) => {
        const li = document.createElement('li');
        li.setAttribute('role', 'option');
        li.setAttribute('aria-selected', String(i === selected));
        const badge = document.createElement('span');
        badge.className = 'badge';
        badge.textContent = row.hit ? row.hit.label : 'Go to';
        const main = document.createElement('span');
        main.className = 'main';
        const title = document.createElement('span');
        title.className = 'title';
        const sub = document.createElement('span');
        sub.className = 'sub';
        if (row.hit) {
            title.textContent = row.hit.key ? `${row.hit.key} ${row.hit.title}` : row.hit.title;
            sub.textContent = row.hit.context ?? '';
        } else if (row.place) {
            title.textContent = row.place.title;
            sub.textContent = [row.place.kind, row.place.subtitle].filter(Boolean).join(' · ');
        }
        main.append(title, sub);
        li.append(badge, main);
        li.addEventListener('mousemove', () => {
            if (selected !== i) {
                selected = i;
                render();
            }
        });
        li.addEventListener('click', () => void open(row));
        list.append(li);
    });
    list.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
    if (fresh && rows.length) stagger(list);
}

async function search(): Promise<void> {
    const mine = ++seq;
    const text = q.value.trim();
    if (!ready) return;
    if (text.length < 2) {
        empty('Type to search. Enter opens, ⌘C copies a snippet, ⌘T starts the timer on an issue.');
        return;
    }
    try {
        const view = await invoke<SearchView>('switcher_search', { q: text, filter: filter || null });
        if (mine !== seq) return; // a newer search is on its way
        rows = [...view.places.map((place) => ({ place })), ...view.results.map((hit) => ({ hit }))];
        selected = 0;
        if (rows.length === 0) empty('Nothing found.');
        else render();
        setStatus(view.total > view.results.length ? `${view.total} results` : '');
    } catch (e) {
        if (mine === seq) {
            empty(errorMessage(e));
        }
    }
}

function schedule(): void {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void search(), 150);
}

async function open(row: Row | undefined): Promise<void> {
    const url = row?.hit?.url ?? row?.place?.url;
    if (!url) {
        setStatus('This result has no page to open.', 'error');
        return;
    }
    try {
        await invoke('switcher_open', { url });
    } catch (e) {
        setStatus(errorMessage(e), 'error');
    }
}

async function copy(row: Row | undefined): Promise<void> {
    if (!row?.hit?.can_copy) {
        setStatus('Choose a snippet to copy.', 'error');
        return;
    }
    try {
        const message = await invoke<string>('switcher_copy_snippet', { id: row.hit.source_id });
        setStatus(message, 'ok');
        window.setTimeout(() => void invoke('switcher_close'), 500);
    } catch (e) {
        setStatus(errorMessage(e), 'error');
    }
}

async function startTimer(row: Row | undefined): Promise<void> {
    if (!row?.hit?.can_time) {
        setStatus('Choose an issue to start the timer.', 'error');
        return;
    }
    try {
        setStatus(await invoke<string>('switcher_start_timer', { issueId: row.hit.source_id }), 'ok');
    } catch (e) {
        setStatus(errorMessage(e), 'error');
    }
}

async function refresh(): Promise<void> {
    q.focus();
    q.select();
    try {
        const ctx = await invoke<Context>('switcher_context');
        const mod = ctx.platform === 'macos' ? '⌘' : 'Ctrl+';
        $('copy-keys').textContent = `${mod}C`;
        $('timer-keys').textContent = `${mod}T`;
        ready = Boolean(ctx.server && ctx.signed_in);
        if (!ctx.server) empty('Connect Brainitive to a server first.');
        else if (!ctx.signed_in) empty(`Sign in to ${ctx.server} first.`);
        else void search();
    } catch (e) {
        empty(errorMessage(e));
    }
}

q.addEventListener('input', schedule);
document.querySelectorAll<HTMLButtonElement>('[data-filter]').forEach((button) =>
    button.addEventListener('click', () => {
        filter = button.dataset.filter ?? '';
        document
            .querySelectorAll<HTMLButtonElement>('[data-filter]')
            .forEach((b) => b.setAttribute('aria-pressed', String(b === button)));
        q.focus();
        void search();
    }),
);

document.addEventListener('keydown', (e) => {
    const mod = e.metaKey || e.ctrlKey;
    if (e.key === 'Escape') {
        e.preventDefault();
        void invoke('switcher_close');
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        if (rows.length === 0) return;
        selected = (selected + (e.key === 'ArrowDown' ? 1 : rows.length - 1)) % rows.length;
        render();
    } else if (e.key === 'Enter') {
        e.preventDefault();
        void open(rows[selected]);
    } else if (mod && e.key.toLowerCase() === 'c' && !window.getSelection()?.toString() && q.selectionStart === q.selectionEnd) {
        e.preventDefault();
        void copy(rows[selected]);
    } else if (mod && e.key.toLowerCase() === 't') {
        e.preventDefault();
        void startTimer(rows[selected]);
    }
});

onShown(() => void refresh());
void refresh();
