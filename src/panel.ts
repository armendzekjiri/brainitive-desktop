// The menu-bar panel: Today, My Day, assigned work, notifications and the timer, under the tray /
// menu-bar icon. Everything goes through the panel's own Rust commands (capabilities/panel.json):
// Rust calls the server with the stored app token, keeps the answers and only acts on ids they
// contain. This page never sees a token or a URL; clicks come back as references (tab + id).
// Server text is set with textContent only.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { $, errorMessage, onShown } from './native';

type Tab = 'today' | 'my_day' | 'assigned' | 'notifications' | 'timer';
const TABS: Tab[] = ['today', 'my_day', 'assigned', 'notifications', 'timer'];

interface Head {
    state: 'ready' | 'signed_out' | 'no_server';
    server: string | null;
    status: 'online' | 'offline' | 'syncing' | 'pending';
    pending: number;
    unread: number;
    tab: Tab;
    platform: 'macos' | 'windows' | 'linux';
    offline_hint: string | null;
    // the saved servers for the header's switcher (ids and names only)
    servers: ServerChoice[];
}

interface ServerChoice {
    id: string;
    name: string;
    host: string;
    current: boolean;
    shortcut: string | null;
}

interface WorkRow {
    id: string;
    key: string | null;
    title: string;
    done: boolean;
    pending: boolean;
    due_date: string | null;
    due_time: string | null;
    overdue: boolean;
    project: string | null;
    color: string | null;
    timeable: boolean;
    has_link: boolean;
}

interface EventRow {
    id: string;
    title: string;
    all_day: boolean;
    start: string | null;
    end: string | null;
    location: string | null;
    now: boolean;
    color: string | null;
    call: boolean;
    has_link: boolean;
}

interface TodayView {
    date: string | null;
    events: EventRow[];
    more_events: number;
    due: WorkRow[];
    more_due: number;
    calendar_problem: boolean;
    without_events: boolean;
}

interface MyDayView {
    date: string | null;
    items: WorkRow[];
    done: number;
    total: number;
    carry_over: number;
}

interface AssignedView {
    groups: { project: string; color: string | null; items: WorkRow[] }[];
    total: number;
    more: boolean;
}

interface NoteRow {
    id: string;
    text: string;
    // marked read offline, waiting to sync
    pending: boolean;
    body: string | null;
    key: string | null;
    context: string | null;
    read: boolean;
    created_at: string | null;
    has_link: boolean;
}

interface Activity {
    id: number;
    name: string;
    color: string | null;
}

interface RunningTimer {
    label: string;
    key: string | null;
    title: string;
    description: string | null;
    // started offline, waiting to sync
    pending: boolean;
    // started offline and not on the server yet: discard works, stop waits for the sync
    started_offline: boolean;
    started_at: string | null;
    elapsed_seconds: number;
    activity_required: boolean;
    default_activity_id: number | null;
    activities: Activity[];
    has_link: boolean;
}

interface TimerView {
    running: RunningTimer | null;
    entries: { id: string; minutes: number; label: string; activity: string | null; note: string | null; from_timer: boolean }[] | null;
    total_minutes: number;
    // seconds logged today without the running timer (newer servers; else `entries`)
    today_seconds: number | null;
    candidates: { id: string; label: string; in_my_day: boolean }[];
    description_supported: boolean;
    // a stop / discard waits to sync
    pending_note: string | null;
    // a stop made offline needs an activity
    activity_needed: { message: string; label: string; activities: Activity[] } | null;
}

interface TabAnswer {
    tab: Tab;
    offline: boolean;
    error: string | null;
    age: number | null;
    from_offline_copy: boolean;
    // timer / mark-read changes are kept and sent later while offline
    queue_offline: boolean;
    today: TodayView | null;
    my_day: MyDayView | null;
    assigned: AssignedView | null;
    notifications: { items: NoteRow[]; unread: number } | null;
    timer: TimerView | null;
}

interface StopOutcome {
    stopped: boolean;
    needs_activity: boolean;
    message: string;
}

type OpenTarget =
    | { kind: 'work' | 'event' | 'notification'; id: string }
    | { kind: 'timer' | 'my_work' | 'notifications' | 'calendar' };

const panel = $('panel');
const signedOut = $('signed-out');
const inner = $('inner');
const content = $('content');

let head: Head | null = null;
let tab: Tab = 'today';
const answers = new Map<Tab, TabAnswer>();
/** When the shown timer answer arrived (the clock ticks on from it). */
let timerLoadedAt = 0;
/** Stop asked for an activity: the choice is shown with this message. */
let activityPrompt: string | null = null;
/** Discard waits for a second click. */
let discardArmed = false;
let timerQuery = '';
/** The optional "what are you doing" of the next timer (newer servers). */
let timerDescription = '';
/** A message after an action (timer stopped, a failed tick). */
let notice: { text: string; error: boolean } | null = null;
let loading = 0;

// ── small DOM helpers ────────────────────────────────────────────────────────

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string | null): HTMLElementTagNameMap[K] {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text != null) node.textContent = text;
    return node;
}

const SVG = 'http://www.w3.org/2000/svg';

/** A small line icon for empty states (static paths, no server data). */
function icon(paths: string[]): SVGSVGElement {
    const svg = document.createElementNS(SVG, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('aria-hidden', 'true');
    for (const d of paths) {
        const p = document.createElementNS(SVG, 'path');
        p.setAttribute('d', d);
        svg.append(p);
    }
    return svg;
}

const ICONS = {
    calm: ['M4 15c2.5-3 5.5-3 8 0s5.5 3 8 0', 'M4 10c2.5-3 5.5-3 8 0s5.5 3 8 0'],
    sun: ['M12 8.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7Z', 'M12 3v2M12 19v2M3 12h2M19 12h2M5.6 5.6 7 7M17 17l1.4 1.4M5.6 18.4 7 17M17 7l1.4-1.4'],
    check: ['M4.5 12.5 9.5 17.5 19.5 6.5'],
    bell: ['M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15Z', 'M10 20.5a2.2 2.2 0 0 0 4 0'],
    cloud: ['M7 18h10a4 4 0 0 0 .5-8 5.5 5.5 0 0 0-10.6 1.5A3.3 3.3 0 0 0 7 18Z', 'M4 4l16 16'],
};

function empty(paths: string[], title: string, line: string, action?: { label: string; run: () => void }): HTMLElement {
    const box = el('div', 'empty');
    box.append(icon(paths), el('strong', undefined, title), el('span', undefined, line));
    if (action) {
        const b = el('button', 'link', action.label);
        b.type = 'button';
        b.addEventListener('click', action.run);
        box.append(b);
    }
    return box;
}

function skeleton(): HTMLElement {
    const box = el('div', 'skeleton');
    for (let i = 0; i < 6; i++) box.append(el('div'));
    return box;
}

function section(title: string, count?: string, action?: { label: string; run: () => void; disabled?: boolean }): HTMLElement {
    const s = el('div', 'section', title);
    if (count) s.append(el('span', 'count', count));
    if (action) {
        const b = el('button', 'action', action.label);
        b.type = 'button';
        b.disabled = !!action.disabled;
        b.addEventListener('click', action.run);
        s.append(b);
    }
    return s;
}

// ── formatting ──────────────────────────────────────────────────────────────

const timeFormat = new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' });
const dayFormat = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' });

function clock(seconds: number): string {
    const s = Math.max(0, Math.floor(seconds));
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = s % 60;
    const two = (n: number) => String(n).padStart(2, '0');
    return h > 0 ? `${h}:${two(m)}:${two(sec)}` : `${m}:${two(sec)}`;
}

function minutes(total: number): string {
    const h = Math.floor(total / 60);
    const m = total % 60;
    if (h && m) return `${h}h ${m}m`;
    if (h) return `${h}h`;
    return `${m}m`;
}

function ago(iso: string | null): string {
    if (!iso) return '';
    const then = Date.parse(iso);
    if (Number.isNaN(then)) return '';
    const s = Math.max(0, (Date.now() - then) / 1000);
    if (s < 60) return 'now';
    if (s < 3600) return `${Math.floor(s / 60)}m`;
    if (s < 86400) return `${Math.floor(s / 3600)}h`;
    if (s < 7 * 86400) return `${Math.floor(s / 86400)}d`;
    return dayFormat.format(new Date(then));
}

function inMinutes(iso: string | null): string {
    if (!iso) return '';
    const start = Date.parse(iso);
    const m = Math.round((start - Date.now()) / 60000);
    if (m <= 0) return '';
    if (m < 60) return `in ${m} min`;
    const h = Math.floor(m / 60);
    return `in ${h} h${m % 60 ? ` ${m % 60} min` : ''}`;
}

function dueLabel(w: WorkRow, today: string | null): string | null {
    if (!w.due_date) return null;
    if (w.due_date === today) return w.due_time ? `Today ${w.due_time}` : 'Today';
    const [y = 0, m = 1, d = 1] = w.due_date.split('-').map(Number);
    return dayFormat.format(new Date(y, m - 1, d));
}

function modKey(): string {
    return head?.platform === 'macos' ? '⌘' : 'Ctrl+';
}

// ── rows ────────────────────────────────────────────────────────────────────

function open(target: OpenTarget): void {
    invoke('panel_open', { target }).catch((e) => showNotice(errorMessage(e), true));
}

async function setDone(w: WorkRow, done: boolean): Promise<void> {
    try {
        await invoke('panel_set_done', { workId: w.id, done });
    } catch (e) {
        showNotice(errorMessage(e), true);
    }
    await load(tab, false);
}

function workRow(w: WorkRow, opts: { today: string | null; showProject?: boolean }): HTMLElement {
    const row = el('div', `row${w.done ? ' done' : ''}`);
    const check = el('button', 'check');
    check.type = 'button';
    check.setAttribute('role', 'checkbox');
    check.setAttribute('aria-checked', String(w.done));
    check.setAttribute('aria-label', w.done ? `Mark "${w.title}" not done` : `Mark "${w.title}" done`);
    check.addEventListener('click', () => {
        check.setAttribute('aria-checked', String(!w.done));
        row.classList.toggle('done', !w.done);
        void setDone(w, !w.done);
    });

    const main = el('button', 'row-main');
    main.type = 'button';
    main.dataset.focus = `work:${w.id}`;
    const title = el('span', 'title');
    if (w.key) title.append(el('span', 'key', w.key));
    title.append(document.createTextNode(w.title));
    main.append(title);
    const meta = el('span', 'meta');
    if (w.overdue) meta.append(el('span', 'chip overdue', 'Overdue'));
    if (w.pending) meta.append(el('span', 'chip pending', 'Waiting to sync'));
    const due = dueLabel(w, opts.today);
    if (due) meta.append(el('span', undefined, due));
    if (opts.showProject !== false && w.project) meta.append(el('span', undefined, w.project));
    if (meta.childNodes.length) main.append(meta);
    main.title = w.has_link ? 'Open in Brainitive' : '';
    main.addEventListener('click', () => open({ kind: 'work', id: w.id }));
    row.append(check, main);
    return row;
}

// ── tabs ────────────────────────────────────────────────────────────────────

function renderToday(v: TodayView, a: TabAnswer): void {
    if (!v.events.length && !v.due.length) {
        inner.append(
            empty(
                ICONS.calm,
                'A calm day',
                v.without_events ? 'Nothing due today. Events show when you’re back online.' : 'Nothing on your calendar and nothing due today.',
                { label: 'Open My Work', run: () => open({ kind: 'my_work' }) },
            ),
        );
        return;
    }
    if (v.events.length) {
        inner.append(section('Next', v.more_events ? `+${v.more_events}` : undefined, { label: 'Calendar', run: () => open({ kind: 'calendar' }) }));
        for (const e of v.events) {
            const row = el('div', 'row');
            const when = el('span', 'when', e.all_day ? 'All day' : e.start ? timeFormat.format(new Date(e.start)) : '');
            const bar = el('span', 'bar');
            if (e.color) bar.style.background = e.color;
            const main = el('button', 'row-main');
            main.type = 'button';
            main.dataset.focus = `event:${e.id}`;
            main.append(el('span', 'title', e.title));
            const meta = el('span', 'meta');
            if (e.now) meta.append(el('span', 'chip now', 'Now'));
            else if (!e.all_day) {
                const soon = inMinutes(e.start);
                if (soon) meta.append(el('span', undefined, soon));
            }
            if (e.call) meta.append(el('span', undefined, 'Video call'));
            if (e.location) meta.append(el('span', undefined, e.location));
            if (meta.childNodes.length) main.append(meta);
            main.addEventListener('click', () => open({ kind: 'event', id: e.id }));
            row.append(when, bar, main);
            inner.append(row);
        }
    } else if (v.without_events) {
        inner.append(el('p', 'hint', 'Events show when you’re back online.'));
    }
    if (v.calendar_problem) inner.append(el('p', 'hint', 'A connected calendar couldn’t be read. Reconnect it on the web.'));
    if (v.due.length) {
        inner.append(section('Due today', `${v.due.length + v.more_due}`));
        for (const w of v.due) inner.append(workRow(w, { today: v.date }));
        if (v.more_due) inner.append(moreLink(`${v.more_due} more in My Work`));
    }
    void a;
}

function moreLink(text: string): HTMLElement {
    const b = el('button', 'link', text);
    b.type = 'button';
    b.style.margin = '4px 4px 0';
    b.addEventListener('click', () => open({ kind: 'my_work' }));
    return b;
}

function renderMyDay(v: MyDayView): void {
    if (!v.items.length) {
        inner.append(
            empty(ICONS.sun, 'Nothing planned yet', 'Plan your day in My Work and it shows up here.', {
                label: 'Open My Work',
                run: () => open({ kind: 'my_work' }),
            }),
        );
        return;
    }
    inner.append(section(v.done === v.total ? 'All done' : 'Today’s plan', `${v.done} of ${v.total}`, { label: 'My Work', run: () => open({ kind: 'my_work' }) }));
    const progress = el('div', 'progress');
    const fill = el('span');
    fill.style.width = `${v.total ? Math.round((v.done / v.total) * 100) : 0}%`;
    progress.append(fill);
    inner.append(progress);
    for (const w of v.items) inner.append(workRow(w, { today: v.date }));
    if (v.carry_over) {
        const n = v.carry_over;
        inner.append(moreLink(`${n} unfinished from earlier days · Bring over in My Work`));
    }
}

function renderAssigned(v: AssignedView): void {
    if (!v.total) {
        inner.append(empty(ICONS.check, 'All clear', 'Nothing open is assigned to you.'));
        return;
    }
    const today = answers.get('today')?.today?.date ?? answers.get('my_day')?.my_day?.date ?? null;
    for (const g of v.groups) {
        const s = section(g.project, `${g.items.length}`);
        const dot = el('span', 'dot-color');
        if (g.color) dot.style.background = g.color;
        s.prepend(dot);
        inner.append(s);
        for (const w of g.items) inner.append(workRow(w, { today, showProject: false }));
    }
    if (v.more) inner.append(moreLink('Show all in My Work'));
}

function renderNotifications(v: { items: NoteRow[]; unread: number }, blocked: boolean): void {
    const unread = v.unread;
    inner.append(
        section(unread ? `${unread} unread` : 'Notifications', undefined, {
            label: 'Mark all read',
            disabled: blocked || (!unread && v.items.every((n) => n.read)),
            run: () => {
                invoke('panel_mark_all_read')
                    .then(() => load('notifications', false))
                    .catch((e) => showNotice(errorMessage(e), true));
            },
        }),
    );
    if (!v.items.length) {
        inner.append(empty(ICONS.bell, 'You’re all caught up', 'New mentions, assignments and replies show up here.'));
        return;
    }
    for (const n of v.items) {
        const row = el('div', `row${n.read ? '' : ' unread'}`);
        const dot = el('span', `unread-dot${n.read ? ' read' : ''}`);
        const main = el('button', 'row-main');
        main.type = 'button';
        main.dataset.focus = `note:${n.id}`;
        main.append(el('span', 'title note-text', n.text));
        const meta = el('span', 'meta');
        if (n.pending) meta.append(el('span', 'chip pending', 'Waiting to sync'));
        if (n.body) meta.append(el('span', undefined, n.body));
        else if (n.context) meta.append(el('span', undefined, n.context));
        if (meta.childNodes.length) main.append(meta);
        main.addEventListener('click', () => open({ kind: 'notification', id: n.id }));
        row.append(dot, main, el('span', 'time-ago', ago(n.created_at)));
        inner.append(row);
    }
    const all = el('button', 'link', 'All notifications');
    all.type = 'button';
    all.style.margin = '4px 4px 0';
    all.addEventListener('click', () => open({ kind: 'notifications' }));
    inner.append(all);
}

/** The inline activity choice: Stop asked for one, or a stop made offline needs one. */
function activityChoice(message: string, activities: Activity[]): HTMLElement {
    const box = el('div');
    box.append(el('p', 'hint', message));
    const list = el('div', 'activities');
    for (const a of activities) {
        const b = el('button', 'btn');
        b.type = 'button';
        const dot = el('span', 'dot-color');
        if (a.color) dot.style.background = a.color;
        b.append(dot, document.createTextNode(a.name));
        b.addEventListener('click', () => void stopTimer(a.id));
        list.append(b);
    }
    if (!activities.length) list.append(el('span', 'hint', 'No activities to choose from. Stop it in the web app.'));
    box.append(list);
    return box;
}

function renderTimer(v: TimerView, blocked: boolean): void {
    const hint = head?.offline_hint ?? null;
    const t = v.running;
    if (v.pending_note) inner.append(el('div', 'banner', v.pending_note));
    const needed = v.activity_needed;
    if (needed && !t) {
        // the timer runs on on the server until the stop goes out with an activity
        const card = el('div', 'timer-card');
        card.append(el('span', 'title', needed.label), activityChoice(needed.message, needed.activities));
        inner.append(card);
    }
    if (t) {
        const card = el('div', 'timer-card');
        const c = el('div', 'clock', clock(t.elapsed_seconds));
        c.id = 'clock';
        c.dataset.elapsed = String(t.elapsed_seconds);
        const label = el('button', 'row-main timer-label');
        label.type = 'button';
        label.dataset.focus = 'timer';
        label.append(el('span', 'title', t.label));
        const meta = el('span', 'meta');
        if (t.pending) meta.append(el('span', 'chip pending', 'Waiting to sync'));
        if (t.description) meta.append(el('span', undefined, t.description));
        if (meta.childNodes.length) label.append(meta);
        label.addEventListener('click', () => open({ kind: 'timer' }));
        const actions = el('div', 'timer-actions');
        const stop = el('button', 'btn primary', 'Stop & log');
        stop.type = 'button';
        stop.disabled = blocked || t.started_offline;
        if (t.started_offline) stop.title = 'Started offline: it can be stopped once it has synced';
        stop.addEventListener('click', () => void stopTimer(null));
        const discard = el('button', 'btn danger', discardArmed ? 'Discard? Click again' : 'Discard');
        discard.type = 'button';
        discard.disabled = blocked;
        discard.addEventListener('click', () => void discardTimer());
        actions.append(stop, discard);
        card.append(c, label, actions);
        if (needed) card.append(activityChoice(needed.message, needed.activities));
        else if (activityPrompt && !blocked) card.append(activityChoice(activityPrompt, t.activities));
        if (blocked && hint) card.append(el('p', 'hint', hint));
        inner.append(card);
    } else {
        inner.append(section('Start a timer'));
        if (v.description_supported) {
            const what = el('input', 'search');
            what.type = 'text';
            what.maxLength = 255;
            what.placeholder = 'What are you working on? (optional)';
            what.value = timerDescription;
            what.disabled = blocked || !v.candidates.length;
            what.setAttribute('aria-label', 'Timer description');
            what.dataset.focus = 'timer-description';
            what.addEventListener('input', () => {
                timerDescription = what.value;
            });
            inner.append(what);
        }
        const input = el('input', 'search');
        input.type = 'search';
        input.placeholder = v.candidates.length ? 'Find an issue from My Day or assigned to you' : 'No open issues in My Day or assigned to you';
        input.value = timerQuery;
        input.disabled = blocked || !v.candidates.length;
        input.setAttribute('aria-label', 'Find an issue to time');
        input.dataset.focus = 'timer-search';
        const list = el('div');
        const draw = () => {
            list.replaceChildren();
            const q = timerQuery.trim().toLowerCase();
            const hits = v.candidates.filter((c) => !q || c.label.toLowerCase().includes(q)).slice(0, q ? 8 : 4);
            for (const c of hits) {
                const row = el('div', 'row');
                const b = el('button', 'row-main');
                b.type = 'button';
                b.dataset.focus = `start:${c.id}`;
                b.disabled = blocked;
                b.append(el('span', 'title', c.label));
                if (c.in_my_day) {
                    const meta = el('span', 'meta');
                    meta.append(el('span', undefined, 'My Day'));
                    b.append(meta);
                }
                b.addEventListener('click', () => void startTimer(c.id));
                row.append(b);
                list.append(row);
            }
            if (q && !hits.length) list.append(el('p', 'hint', 'No match. Start timers on other issues from the quick switcher (⌘T).'));
            requestResize();
        };
        input.addEventListener('input', () => {
            timerQuery = input.value;
            draw();
        });
        input.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') {
                const first = list.querySelector<HTMLButtonElement>('button.row-main:not(:disabled)');
                first?.click();
            }
        });
        inner.append(input, list);
        draw();
        if (blocked && hint) inner.append(el('p', 'hint', hint));
    }

    if (v.today_seconds != null) {
        // the server's total of finished logs + the running timer, ticking
        const s = section('Today');
        const total = el('span', 'total', minutes(Math.floor((v.today_seconds + (t && !t.pending ? t.elapsed_seconds : 0)) / 60)));
        total.id = 'today-total';
        total.dataset.logged = String(v.today_seconds);
        total.dataset.running = t && !t.pending ? String(t.elapsed_seconds) : '';
        s.append(total);
        inner.append(s);
        if (!v.today_seconds && !t) inner.append(el('p', 'hint', 'No time logged today yet.'));
    } else if (v.entries) {
        const s = section('Today');
        s.append(el('span', 'total', minutes(v.total_minutes)));
        inner.append(s);
        if (!v.entries.length) inner.append(el('p', 'hint', 'No time logged today yet.'));
        for (const e of v.entries.slice(0, 12)) {
            const row = el('div', 'row');
            const main = el('div', 'row-main');
            main.append(el('span', 'title', e.label));
            const meta = el('span', 'meta');
            if (e.activity) meta.append(el('span', undefined, e.activity));
            if (e.note) meta.append(el('span', undefined, e.note));
            if (meta.childNodes.length) main.append(meta);
            row.append(main, el('span', 'duration', minutes(e.minutes)));
            inner.append(row);
        }
    }
}

async function startTimer(id: string): Promise<void> {
    try {
        await invoke('panel_timer_start', { workId: id, description: timerDescription.trim() || null });
        timerQuery = '';
        timerDescription = '';
        notice = null;
    } catch (e) {
        showNotice(errorMessage(e), true);
    }
    await load('timer', true);
}

async function stopTimer(activityId: number | null): Promise<void> {
    try {
        const out = await invoke<StopOutcome>('panel_timer_stop', { activityId });
        activityPrompt = out.needs_activity ? out.message : null;
        if (out.stopped) showNotice(out.message, false);
    } catch (e) {
        showNotice(errorMessage(e), true);
    }
    await load('timer', true);
}

async function discardTimer(): Promise<void> {
    if (!discardArmed) {
        discardArmed = true;
        render();
        window.setTimeout(() => {
            discardArmed = false;
            render();
        }, 3000);
        return;
    }
    discardArmed = false;
    activityPrompt = null;
    try {
        await invoke('panel_timer_discard');
        showNotice('Timer discarded.', false);
    } catch (e) {
        showNotice(errorMessage(e), true);
    }
    await load('timer', true);
}

// ── page ────────────────────────────────────────────────────────────────────

function showNotice(text: string, error: boolean): void {
    notice = { text, error };
    render();
    window.setTimeout(() => {
        if (notice?.text === text) {
            notice = null;
            render();
        }
    }, 4000);
}

function banner(a: TabAnswer): void {
    if (notice) {
        inner.append(el('div', `banner${notice.error ? ' error' : ''}`, notice.text));
    }
    if (a.offline || a.from_offline_copy) {
        const since = a.from_offline_copy ? 'from this computer’s offline copy' : a.age != null ? `loaded ${ageText(a.age)}` : 'loaded last';
        const b = el('div', 'banner', `Offline — showing what was ${since}`);
        inner.append(b);
    } else if (a.error) {
        const b = el('div', 'banner error', a.error);
        const retry = el('button', 'link', 'Try again');
        retry.type = 'button';
        retry.addEventListener('click', () => void load(tab, true));
        b.append(retry);
        inner.append(b);
    }
}

function ageText(seconds: number): string {
    if (seconds < 90) return 'just now';
    if (seconds < 3600) return `${Math.round(seconds / 60)} min ago`;
    return `${Math.round(seconds / 3600)} h ago`;
}

function render(): void {
    const focused = (document.activeElement as HTMLElement | null)?.dataset?.focus;
    const scroll = content.scrollTop;
    inner.replaceChildren();
    const a = answers.get(tab);
    if (a) banner(a);
    const offline = !!a?.offline;
    // offline, timer / mark-read work only where the server keeps them for later (newer servers)
    const blocked = offline && !a?.queue_offline;
    const has = a && (a.today || a.my_day || a.assigned || a.notifications || a.timer);
    if (!has) {
        if (loading > 0 || !a) inner.append(skeleton());
        else if (offline) inner.append(empty(ICONS.cloud, 'Offline', 'This shows again once Brainitive can reach your server.'));
        else if (!a.error) inner.append(empty(ICONS.calm, 'Nothing here', 'Nothing to show right now.'));
    } else if (a.today) renderToday(a.today, a);
    else if (a.my_day) renderMyDay(a.my_day);
    else if (a.assigned) renderAssigned(a.assigned);
    else if (a.notifications) renderNotifications(a.notifications, blocked);
    else if (a.timer) {
        if (!a.timer.running) activityPrompt = null;
        renderTimer(a.timer, blocked);
    }
    content.scrollTop = scroll;
    if (focused) {
        const again = inner.querySelector<HTMLElement>(`[data-focus="${CSS.escape(focused)}"]`);
        again?.focus({ preventScroll: true });
    }
    requestResize();
}

// ── the server switcher ("Work ▾") ───────────────────────────────────────────

const serverMenu = $('server-menu');
const hostButton = $<HTMLButtonElement>('host');

function switchServer(id: string): void {
    closeServerMenu();
    invoke('panel_switch_server', { id }).catch((e) => showNotice(errorMessage(e), true));
}

function closeServerMenu(focusHost = false): void {
    if (serverMenu.hidden) return;
    serverMenu.hidden = true;
    hostButton.setAttribute('aria-expanded', 'false');
    if (focusHost) hostButton.focus();
}

function openServerMenu(): void {
    const servers = head?.servers ?? [];
    const items: HTMLElement[] = servers.map((s) => {
        const b = document.createElement('button');
        b.type = 'button';
        b.setAttribute('role', 'menuitemradio');
        b.setAttribute('aria-checked', String(s.current));
        b.title = s.host;
        const check = document.createElement('span');
        check.className = 'mark';
        check.textContent = s.current ? '✓' : '';
        const label = document.createElement('span');
        label.className = 'label';
        label.textContent = s.name;
        b.append(check, label);
        if (s.shortcut) {
            const keys = document.createElement('span');
            keys.className = 'keys';
            keys.textContent = s.shortcut;
            b.append(keys);
        }
        b.addEventListener('click', () => (s.current ? closeServerMenu(true) : switchServer(s.id)));
        return b;
    });
    if (items.length) items.push(document.createElement('hr'));
    const manage = document.createElement('button');
    manage.type = 'button';
    manage.setAttribute('role', 'menuitem');
    manage.append(Object.assign(document.createElement('span'), { className: 'mark' }), 'Manage Servers…');
    manage.addEventListener('click', () => {
        closeServerMenu();
        void invoke('panel_run', { action: 'manage_servers' }).catch(() => {});
    });
    items.push(manage);
    serverMenu.replaceChildren(...items);
    serverMenu.hidden = false;
    hostButton.setAttribute('aria-expanded', 'true');
    (serverMenu.querySelector<HTMLElement>('[aria-checked="true"]') ?? serverMenu.querySelector<HTMLElement>('button'))?.focus();
}

hostButton.addEventListener('click', () => (serverMenu.hidden ? openServerMenu() : closeServerMenu(true)));
document.addEventListener('mousedown', (e) => {
    if (!serverMenu.hidden && !serverMenu.contains(e.target as Node) && !hostButton.contains(e.target as Node)) closeServerMenu();
});

/** Signed out of this server: the other saved servers are one click away. */
function renderSignedOutServers(): void {
    const box = $('so-servers');
    const others = (head?.servers ?? []).filter((s) => !s.current);
    box.hidden = others.length === 0;
    box.replaceChildren(
        ...others.map((s) => {
            const b = document.createElement('button');
            b.type = 'button';
            b.textContent = `Switch to ${s.name}`;
            b.addEventListener('click', () => switchServer(s.id));
            return b;
        }),
    );
}

function renderHead(): void {
    if (!head) return;
    const ready = head.state === 'ready';
    panel.hidden = !ready;
    signedOut.hidden = ready;
    renderSignedOutServers();
    if (!ready) {
        $('so-title').textContent = head.state === 'no_server' ? 'Connect to Brainitive' : 'Sign in to Brainitive';
        $('so-text').textContent =
            head.state === 'no_server'
                ? 'Choose Brainitive Cloud or your own server to see your day here.'
                : 'See today, your work, notifications and the timer right here.';
        requestResize();
        return;
    }
    $('host-name').textContent = head.server ?? 'Brainitive';
    const dot = $('dot');
    dot.className = `dot ${head.status}`;
    const sync = $('sync');
    const labels: Record<Head['status'], string> = {
        online: 'Up to date',
        offline: head.pending ? `Offline · ${head.pending} waiting` : 'Offline',
        syncing: 'Syncing…',
        pending: `${head.pending} waiting to sync`,
    };
    sync.textContent = labels[head.status];
    dot.title = labels[head.status];
    const badge = $('badge');
    badge.hidden = !head.unread;
    badge.textContent = head.unread > 99 ? '99+' : String(head.unread);
    $('tab-notifications').setAttribute('aria-label', head.unread ? `Inbox, ${head.unread} unread` : 'Inbox');
    TABS.forEach((t, i) => {
        const b = $(`tab-${t}`);
        b.title = `${b.textContent?.trim()} (${modKey()}${i + 1})`;
    });
}

function selectTab(next: Tab, focusTab = false): void {
    tab = next;
    for (const t of TABS) {
        const b = $(`tab-${t}`);
        const on = t === next;
        b.setAttribute('aria-selected', String(on));
        b.tabIndex = on ? 0 : -1;
        if (on && focusTab) b.focus();
    }
    content.scrollTop = 0;
    discardArmed = false;
    render();
    void invoke('panel_set_tab', { tab: next }).catch(() => {});
    void load(next, true);
}

/** Shows what Rust keeps for `t` at once, then (with `refresh`) asks the server. */
async function load(t: Tab, refresh: boolean): Promise<void> {
    if (head?.state !== 'ready') return;
    if (refresh) {
        if (!answers.has(t)) {
            try {
                answers.set(t, await invoke<TabAnswer>('panel_load', { tab: t, refresh: false }));
            } catch {
                // the refresh below reports
            }
        }
        loading++;
        if (t === tab) render();
    }
    try {
        const a = await invoke<TabAnswer>('panel_load', { tab: t, refresh });
        answers.set(t, a);
        if (t === 'timer') timerLoadedAt = Date.now();
    } catch (e) {
        showNotice(errorMessage(e), true);
    } finally {
        if (refresh) loading--;
    }
    if (t === tab) render();
}

async function refreshHead(): Promise<void> {
    try {
        head = await invoke<Head>('panel_view');
    } catch (e) {
        console.error(errorMessage(e));
        return;
    }
    document.body.classList.toggle('glass', head.platform === 'macos');
    document.body.classList.toggle('solid', head.platform !== 'macos');
    renderHead();
}

// ── size ────────────────────────────────────────────────────────────────────

let resizeQueued = false;
let lastHeight = 0;

function requestResize(): void {
    if (resizeQueued) return;
    resizeQueued = true;
    requestAnimationFrame(() => {
        resizeQueued = false;
        let height: number;
        if (!signedOut.hidden) {
            height = 260;
        } else {
            const chrome = panel.scrollHeight - content.clientHeight;
            height = chrome + inner.scrollHeight;
        }
        height = Math.ceil(height);
        if (Math.abs(height - lastHeight) < 2) return;
        lastHeight = height;
        void invoke('panel_resize', { height }).catch(() => {});
    });
}

// ── keyboard ────────────────────────────────────────────────────────────────

function focusables(): HTMLElement[] {
    return Array.from(inner.querySelectorAll<HTMLElement>('button.row-main:not(:disabled), input.search:not(:disabled)'));
}

document.addEventListener('keydown', (e) => {
    if (!serverMenu.hidden) {
        // the server list has the keys while it is open
        const items = Array.from(serverMenu.querySelectorAll<HTMLElement>('button'));
        const i = items.indexOf(document.activeElement as HTMLElement);
        if (e.key === 'Escape') {
            e.preventDefault();
            closeServerMenu(true);
            return;
        }
        if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault();
            const next = e.key === 'ArrowDown' ? (i + 1) % items.length : (i - 1 + items.length) % items.length;
            items[next]?.focus();
            return;
        }
        if (e.key === 'Tab') closeServerMenu();
    }
    if (e.key === 'Escape') {
        e.preventDefault();
        if (activityPrompt) {
            activityPrompt = null;
            render();
            return;
        }
        void invoke('panel_close');
        return;
    }
    const mod = head?.platform === 'macos' ? e.metaKey : e.ctrlKey;
    if (mod && e.key >= '1' && e.key <= '5') {
        e.preventDefault();
        selectTab(TABS[Number(e.key) - 1] ?? 'today', true);
        return;
    }
    const target = e.target as HTMLElement;
    const onTab = target.classList.contains('tab');
    if (onTab && (e.key === 'ArrowRight' || e.key === 'ArrowLeft')) {
        e.preventDefault();
        const i = TABS.indexOf(tab);
        const next = TABS[(i + (e.key === 'ArrowRight' ? 1 : TABS.length - 1)) % TABS.length] ?? 'today';
        selectTab(next, true);
        return;
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        const list = focusables();
        if (!list.length) return;
        e.preventDefault();
        const i = list.indexOf(target);
        let next: HTMLElement | undefined;
        if (onTab || i < 0) next = e.key === 'ArrowDown' ? list[0] : list[list.length - 1];
        else if (e.key === 'ArrowUp' && i === 0) {
            $(`tab-${tab}`).focus();
            return;
        } else next = list[Math.min(list.length - 1, Math.max(0, i + (e.key === 'ArrowDown' ? 1 : -1)))];
        next?.focus();
        next?.scrollIntoView({ block: 'nearest' });
    }
});

// ── wiring ──────────────────────────────────────────────────────────────────

for (const t of TABS) {
    $(`tab-${t}`).addEventListener('click', () => selectTab(t));
}
for (const b of document.querySelectorAll<HTMLButtonElement>('[data-run]')) {
    b.addEventListener('click', () => void invoke('panel_run', { action: b.dataset.run }).catch(() => {}));
}
$('so-button').addEventListener('click', () => void invoke('panel_run', { action: 'sign_in' }).catch(() => {}));

// the running timer ticks
window.setInterval(() => {
    const since = (Date.now() - timerLoadedAt) / 1000;
    const c = document.getElementById('clock');
    if (c) c.textContent = clock(Number(c.dataset.elapsed ?? 0) + since);
    const total = document.getElementById('today-total');
    if (total && total.dataset.running) {
        const seconds = Number(total.dataset.logged ?? 0) + Number(total.dataset.running) + since;
        total.textContent = minutes(Math.floor(seconds / 60));
    }
}, 1000);

/** Opened (again): new data, the remembered tab on the first open. */
let opened = false;
async function shown(): Promise<void> {
    await refreshHead();
    if (!head || head.state !== 'ready') return;
    if (!opened) {
        opened = true;
        selectTab(head.tab);
    } else {
        void load(tab, true);
    }
}

onShown(() => void shown());
void listen('panel://changed', async () => {
    await refreshHead();
    if (head?.state === 'ready') {
        if (!opened) return void shown();
        await load(tab, false);
    }
});
void shown();
