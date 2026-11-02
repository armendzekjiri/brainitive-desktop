// The offline window (BRAI-366): this computer's copy of My Day, My Work, the kept task lists and
// the capture inbox. Reading and every change go through the window's own Rust commands
// (capabilities/offline.json); Rust queues changes and sends them with the stored app token, so
// this page never sees a token or calls a server. Server text is set with textContent only.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { $, errorMessage, onShown } from './native';

interface Work {
    id: string;
    key: string | null;
    title: string;
    done: boolean;
    due_date: string | null;
    project: string | null;
    source: string | null;
    pending: boolean;
    in_my_day: boolean;
}

interface Task {
    id: string;
    title: string;
    done: boolean;
    due_date: string | null;
    pending: boolean;
}

interface List {
    id: number;
    name: string;
    project: string | null;
    can_manage_items?: boolean;
    tasks: Task[];
}

interface Capture {
    id: string;
    text: string;
    url: string | null;
    captured_at: string | null;
    pending: boolean;
}

interface Problem {
    request_id: string;
    kind: string;
    title: string;
    message: string;
    mine: string | null;
    theirs: string | null;
    at: number;
}

interface View {
    server: string | null;
    signed_in: boolean;
    online: boolean | null;
    syncing: boolean;
    last_error: string | null;
    platform: string;
    // the data (absent when signed out)
    has_data?: boolean;
    user_name?: string | null;
    synced_at?: number | null;
    pending?: number;
    my_day?: { date: string; items: Work[] } | null;
    work?: Work[];
    lists?: List[];
    captures?: Capture[];
    problems?: Problem[];
}

type Tab = 'my-day' | 'work' | 'lists' | 'captures' | 'problems';

let view: View | null = null;
let tab: Tab = 'my-day';
let listId: number | null = null;
/** A task is being renamed: a refresh must not throw the input away. */
let editing = false;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, text?: string, className?: string): HTMLElementTagNameMap[K] {
    const node = document.createElement(tag);
    if (text !== undefined) node.textContent = text;
    if (className) node.className = className;
    return node;
}

function showError(id: string, message: string | null): void {
    const box = $(id);
    box.textContent = message ?? '';
    box.hidden = !message;
}

function ago(seconds: number | null | undefined): string {
    if (!seconds) return 'not synced yet';
    const diff = Math.max(0, Math.round(Date.now() / 1000 - seconds));
    if (diff < 60) return 'synced just now';
    if (diff < 3600) return `synced ${Math.round(diff / 60)} min ago`;
    if (diff < 86400) return `synced ${Math.round(diff / 3600)} h ago`;
    return `synced on ${new Date(seconds * 1000).toLocaleDateString()}`;
}

function plural(n: number, word: string): string {
    return `${n} ${word}${n === 1 ? '' : 's'}`;
}

function renderStatus(v: View): void {
    const status = $('status');
    const offline = v.online === false;
    status.className = `status ${offline ? 'offline' : v.online ? 'online' : ''}`;
    const pending = v.pending ?? 0;
    let title: string;
    if (!v.signed_in) title = v.server ? `Signed out of ${v.server}` : 'Not connected';
    else if (v.syncing) title = 'Syncing…';
    else if (offline) title = 'Offline — changes will sync';
    else if (pending > 0) title = `${plural(pending, 'change')} waiting to sync`;
    else title = 'Up to date';
    $('status-title').textContent = title;
    const parts = [v.server, v.user_name, v.signed_in ? ago(v.synced_at) : null];
    if (offline && pending > 0) parts.push(`${plural(pending, 'change')} kept on this computer`);
    $('status-detail').textContent = parts.filter(Boolean).join(' · ');
    ($('sync-now') as HTMLButtonElement).disabled = !v.signed_in || v.syncing;
    showError('sync-error', v.signed_in ? v.last_error : null);
    document.title = offline ? 'Brainitive Offline — offline' : 'Brainitive Offline';
}

function pendingBadge(): HTMLElement {
    const badge = el('span', 'waiting to sync', 'pending');
    badge.title = 'Saved on this computer, sent when the server can be reached';
    return badge;
}

function workRow(w: Work, opts: { myDayButton: boolean }): HTMLLIElement {
    const li = el('li');
    if (w.done) li.classList.add('done');
    const box = el('input');
    box.type = 'checkbox';
    box.checked = w.done;
    box.setAttribute('aria-label', `Done: ${w.title}`);
    box.addEventListener('change', () => void act('offline_set_done', { workId: w.id, done: box.checked }));
    const title = el('span', undefined, 'title');
    title.append(el('span', w.key ? `${w.key} ${w.title}` : w.title));
    const meta = [w.project, w.source, w.due_date ? `due ${w.due_date}` : null].filter(Boolean).join(' · ');
    if (meta) title.append(el('small', meta));
    li.append(box, title);
    if (w.pending) li.append(pendingBadge());
    if (opts.myDayButton && !w.in_my_day && !w.done) {
        const add = el('button', '+ My Day', 'link');
        add.type = 'button';
        add.addEventListener('click', () => void act('offline_my_day_add', { workId: w.id }));
        li.append(add);
    } else if (opts.myDayButton && w.in_my_day) {
        li.append(el('small', 'in My Day', 'muted'));
    }
    return li;
}

function renderDay(v: View): void {
    const list = $('day-list');
    list.replaceChildren();
    const items = v.my_day?.items ?? [];
    for (const w of items) list.append(workRow(w, { myDayButton: false }));
    $('day-empty').hidden = items.length > 0;
    $('day-title').textContent = v.my_day ? `My Day · ${v.my_day.date}` : 'My Day';
    const done = items.filter((w) => w.done).length;
    setCount('my-day', items.length ? `${done}/${items.length}` : '');
}

function renderWork(v: View): void {
    const list = $('work-list');
    list.replaceChildren();
    const work = v.work ?? [];
    for (const w of work) list.append(workRow(w, { myDayButton: true }));
    $('work-empty').hidden = work.length > 0;
    setCount('work', work.length ? String(work.filter((w) => !w.done).length) : '');
}

function taskRow(listIdValue: number, t: Task): HTMLLIElement {
    const li = el('li');
    if (t.done) li.classList.add('done');
    const box = el('input');
    box.type = 'checkbox';
    box.checked = t.done;
    box.setAttribute('aria-label', `Done: ${t.title}`);
    box.addEventListener('change', () =>
        void act('offline_update_task', { taskListId: listIdValue, taskId: t.id, title: null, done: box.checked }),
    );
    const title = el('span', undefined, 'title');
    title.append(el('span', t.title));
    if (t.due_date) title.append(el('small', `due ${t.due_date}`));
    title.title = 'Double-click to rename';
    title.addEventListener('dblclick', () => rename(li, title, listIdValue, t));
    li.append(box, title);
    if (t.pending) li.append(pendingBadge());
    const edit = el('button', 'Rename', 'link');
    edit.type = 'button';
    edit.addEventListener('click', () => rename(li, title, listIdValue, t));
    li.append(edit);
    return li;
}

function rename(li: HTMLLIElement, title: HTMLElement, listIdValue: number, t: Task): void {
    const input = el('input');
    input.type = 'text';
    input.maxLength = 500;
    input.value = t.title;
    let finished = false;
    editing = true;
    const finish = (save: boolean): void => {
        if (finished) return;
        finished = true;
        editing = false;
        const value = input.value.trim();
        if (save && value && value !== t.title) {
            void act('offline_update_task', { taskListId: listIdValue, taskId: t.id, title: value, done: null });
        } else {
            render();
        }
    };
    input.addEventListener('keydown', (ev) => {
        if (ev.key === 'Enter') finish(true);
        if (ev.key === 'Escape') finish(false);
    });
    input.addEventListener('blur', () => finish(true), { once: true });
    li.replaceChild(input, title);
    input.focus();
    input.select();
}

function renderLists(v: View): void {
    const lists = v.lists ?? [];
    const select = $('list-select') as HTMLSelectElement;
    if (listId === null || !lists.some((l) => l.id === listId)) listId = lists[0]?.id ?? null;
    select.replaceChildren(
        ...lists.map((l) => {
            const option = el('option', l.project ? `${l.name} — ${l.project}` : l.name);
            option.value = String(l.id);
            option.selected = l.id === listId;
            return option;
        }),
    );
    const current = lists.find((l) => l.id === listId);
    const tasks = $('task-list');
    tasks.replaceChildren();
    if (current) for (const t of current.tasks) tasks.append(taskRow(current.id, t));
    $('add-task').hidden = !current || current.can_manage_items === false;
    select.hidden = lists.length === 0;
    $('lists-empty').hidden = lists.length > 0;
}

function renderCaptures(v: View): void {
    const list = $('capture-list');
    list.replaceChildren();
    for (const c of v.captures ?? []) {
        const li = el('li');
        const body = el('span', undefined, 'title');
        body.append(el('span', c.text));
        const meta = [c.url, c.captured_at ? new Date(c.captured_at).toLocaleString() : null].filter(Boolean).join(' · ');
        if (meta) body.append(el('small', meta));
        li.append(body);
        if (c.pending) li.append(pendingBadge());
        list.append(li);
    }
}

const KIND_LABEL: Record<string, string> = {
    conflict: 'Changed on the server meanwhile',
    deleted: 'Deleted or out of reach',
    forbidden: 'Not allowed',
    invalid: 'Not accepted',
    waiting: 'Could not be sent',
    rejected: 'Not accepted',
    timer_running: 'Another timer is running',
};

function renderProblems(v: View): void {
    const problems = v.problems ?? [];
    const list = $('problem-list');
    list.replaceChildren();
    for (const p of problems) {
        const li = el('li');
        li.append(el('strong', KIND_LABEL[p.kind] ?? 'Not applied'), el('div', p.title), el('p', p.message, 'muted'));
        if (p.theirs !== null) li.append(el('small', `On the server now: ${p.theirs}`, 'muted'));
        if (p.mine) {
            // the user's own text, so nothing typed offline is lost
            const mine = el('textarea', undefined, 'mine');
            mine.readOnly = true;
            mine.rows = Math.min(6, p.mine.split('\n').length + 1);
            mine.value = p.mine;
            mine.setAttribute('aria-label', 'Your text');
            li.append(mine);
        }
        const row = el('div', undefined, 'row');
        if (p.mine) {
            const copy = el('button', 'Copy my text');
            copy.type = 'button';
            copy.addEventListener('click', () => {
                navigator.clipboard.writeText(p.mine ?? '').then(
                    () => (copy.textContent = 'Copied'),
                    () => (copy.textContent = 'Select the text and copy it'),
                );
            });
            row.append(copy);
        }
        const dismiss = el('button', 'Dismiss');
        dismiss.type = 'button';
        dismiss.addEventListener('click', () => void act('offline_dismiss_problem', { requestId: p.request_id }));
        row.append(dismiss);
        li.append(row);
        list.append(li);
    }
    $('problems-tab').hidden = problems.length === 0;
    setCount('problems', problems.length ? String(problems.length) : '');
    if (tab === 'problems' && problems.length === 0) tab = 'my-day';
}

function setCount(name: string, text: string): void {
    document.querySelectorAll<HTMLElement>(`[data-count="${name}"]`).forEach((n) => (n.textContent = text));
}

function render(): void {
    const v = view;
    if (!v) return;
    renderStatus(v);
    const hasData = v.signed_in && (v.has_data || (v.pending ?? 0) > 0 || (v.problems?.length ?? 0) > 0);
    $('empty').hidden = hasData;
    if (!v.signed_in) {
        $('empty-title').textContent = 'Sign in first';
        $('empty-text').textContent = 'Offline mode keeps the data of the account signed in to this app. Sign in from the main window.';
    }
    document.querySelector<HTMLElement>('.tabs')!.hidden = !hasData;
    if (hasData) {
        renderDay(v);
        renderWork(v);
        renderLists(v);
        renderCaptures(v);
        renderProblems(v);
    }
    document.querySelectorAll<HTMLElement>('[data-panel]').forEach((p) => (p.hidden = !hasData || p.dataset.panel !== tab));
    document.querySelectorAll<HTMLButtonElement>('[data-tab]').forEach((b) => b.setAttribute('aria-selected', String(b.dataset.tab === tab)));
}

async function refresh(): Promise<void> {
    try {
        view = await invoke<View>('offline_view');
        if (!editing) render();
    } catch (e) {
        showError('form-error', errorMessage(e));
    }
}

async function act(command: string, args: Record<string, unknown>): Promise<boolean> {
    showError('form-error', null);
    try {
        view = await invoke<View>(command, args);
        render();
        return true;
    } catch (e) {
        showError('form-error', errorMessage(e));
        await refresh();
        return false;
    }
}

function wire(): void {
    document.querySelectorAll<HTMLButtonElement>('[data-tab]').forEach((b) =>
        b.addEventListener('click', () => {
            tab = b.dataset.tab as Tab;
            render();
        }),
    );
    $('list-select').addEventListener('change', (ev) => {
        listId = Number((ev.target as HTMLSelectElement).value);
        render();
    });
    $('add-task').addEventListener('submit', (ev) => {
        ev.preventDefault();
        const input = $('task-title') as HTMLInputElement;
        if (listId === null || !input.value.trim()) return;
        void act('offline_add_task', { taskListId: listId, title: input.value }).then((ok) => {
            if (ok) input.value = '';
        });
    });
    $('add-capture').addEventListener('submit', (ev) => {
        ev.preventDefault();
        const text = $('capture-text') as HTMLTextAreaElement;
        const url = $('capture-url') as HTMLInputElement;
        if (!text.value.trim()) return;
        void act('offline_add_capture', { text: text.value, url: url.value.trim() || null }).then((ok) => {
            if (ok) {
                text.value = '';
                url.value = '';
            }
        });
    });
    $('sync-now').addEventListener('click', () => void act('offline_sync_now', {}));
    $('open-app').addEventListener('click', () => void invoke('offline_open_app').catch(() => undefined));
    $('dismiss-all').addEventListener('click', () => void act('offline_dismiss_problem', { requestId: null }));
}

wire();
onShown(() => void refresh());
void listen('offline://changed', () => void refresh());
// "synced 3 min ago" stays true
window.setInterval(() => {
    if (view) renderStatus(view);
}, 30_000);
void refresh();
