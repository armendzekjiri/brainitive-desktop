// The New task window: a new issue on a board, from any app (⌃⌘T / Ctrl+Alt+T, the Brainitive
// key + T, the tray, the menu-bar panel). Every call goes to Rust, which talks to the server with
// the app token; this page only gets ids, names and issue keys — never a token or a URL.
// Server-provided text is set with textContent only.

import { invoke } from '@tauri-apps/api/core';
import { $, busy, errorMessage, onShown } from './native';

interface Board {
    id: number;
    name: string;
}

interface Project {
    id: number;
    name: string;
    key: string | null;
    workspace: string;
    boards: Board[];
}

interface View {
    state: 'ready' | 'signed_out' | 'no_server';
    server: string | null;
    platform: 'macos' | 'windows' | 'linux';
    shortcut: string;
    projects: Project[];
    selection: { project_id: number | null; board_id: number | null };
    stale: boolean;
}

interface Choice {
    id: number;
    name: string;
    slug: string;
    color: string | null;
    is_default: boolean;
}

interface ConfigView {
    types: Choice[];
    priorities: Choice[];
    type_id: number | null;
    board_id: number;
}

interface Created {
    key: string | null;
    title: string;
    can_open: boolean;
}

interface TaskError {
    kind: string;
    message: string;
    fields?: Record<string, string>;
    inbox?: boolean;
}

interface CaptureResult {
    status: 'saved' | 'queued';
    message: string;
}

const app = $('app');
const form = $<HTMLFormElement>('form');
const projectInput = $<HTMLInputElement>('project');
const projectList = $<HTMLUListElement>('project-list');
const boardField = $('board-field');
const boardSelect = $<HTMLSelectElement>('board');
const title = $<HTMLInputElement>('title');
const descToggle = $<HTMLButtonElement>('desc-toggle');
const description = $<HTMLTextAreaElement>('description');
const priorities = $('priorities');
const typeField = $('type-field');
const typeSelect = $<HTMLSelectElement>('type');
const status = $('status');
const inboxButton = $<HTMLButtonElement>('inbox');
const createButton = $<HTMLButtonElement>('create');

let view: View | null = null;
let projects: Project[] = [];
let projectId: number | null = null;
let boardId: number | null = null;
let boardTouched = false;
let priorityId: number | null = null;
let typeId: number | null = null;
let config: ConfigView | null = null;
let configSeq = 0;
let sending = false;
// the project list: what matches the typed text, and the highlighted row
let matches: Project[] = [];
let active = 0;

// ── helpers ─────────────────────────────────────────────────────────────────

function asError(e: unknown): TaskError {
    if (e && typeof e === 'object' && 'kind' in e && 'message' in e) return e as TaskError;
    return { kind: 'error', message: errorMessage(e) };
}

function setStatus(message: string, kind: 'ok' | 'error' | 'muted' = 'muted'): void {
    status.textContent = message;
    status.className = `status ${kind}`;
    fit();
}

function clearErrors(): void {
    document.querySelectorAll<HTMLElement>('.field-error').forEach((el) => {
        el.textContent = '';
        el.hidden = true;
    });
    document.querySelectorAll('.invalid').forEach((el) => el.classList.remove('invalid'));
    inboxButton.hidden = true;
    setStatus('');
}

const fieldInput: Record<string, () => HTMLElement> = {
    title: () => title,
    description: () => description,
    project: () => projectInput,
    board: () => boardSelect,
    type: () => typeSelect,
    priority: () => priorities,
};

function showError(e: TaskError): void {
    let shown = false;
    for (const [field, message] of Object.entries(e.fields ?? {})) {
        const el = document.querySelector<HTMLElement>(`.field-error[data-for="${field}"]`);
        if (!el) continue;
        el.textContent = message;
        el.hidden = false;
        fieldInput[field]?.().classList.add('invalid');
        if (field === 'description') openDescription(false);
        shown = true;
    }
    // the sentence goes to the status line unless it only repeats one field's message
    const fields = Object.values(e.fields ?? {});
    if (!shown || fields.length > 1 || fields[0] !== e.message) setStatus(e.message, 'error');
    inboxButton.hidden = !e.inbox;
    if (e.kind === 'signed_out' || e.kind === 'no_server') void refresh();
    fit();
}

/** The window follows the page (Rust clamps it); an open project list may reach below the form. */
function fit(): void {
    requestAnimationFrame(() => {
        // + the window's 1 px border, top and bottom
        let height = app.scrollHeight + 2;
        if (!projectList.hidden) height = Math.max(height, projectList.getBoundingClientRect().bottom + 12);
        void invoke('task_resize', { height }).catch(() => {});
    });
}

function show(section: 'form' | 'done' | 'blocked'): void {
    form.hidden = section !== 'form';
    $('done').hidden = section !== 'done';
    $('blocked').hidden = section !== 'blocked';
    fit();
}

// ── project combobox ────────────────────────────────────────────────────────

function project(): Project | undefined {
    return projects.find((p) => p.id === projectId);
}

function projectLabel(p: Project): string {
    return p.key ? `${p.name} (${p.key})` : p.name;
}

function renderProjectInput(): void {
    const p = project();
    projectInput.value = p ? projectLabel(p) : '';
    projectInput.placeholder = projects.length ? 'Search projects…' : view?.stale ? 'Loading projects…' : 'No projects';
}

function openList(): void {
    filterList(projectInput.value === (project() ? projectLabel(project()!) : '') ? '' : projectInput.value);
    projectList.hidden = false;
    projectInput.setAttribute('aria-expanded', 'true');
    fit();
}

function closeList(): void {
    projectList.hidden = true;
    projectInput.setAttribute('aria-expanded', 'false');
    projectInput.removeAttribute('aria-activedescendant');
    fit();
}

function filterList(query: string): void {
    const q = query.trim().toLowerCase();
    matches = projects.filter(
        (p) => !q || p.name.toLowerCase().includes(q) || (p.key ?? '').toLowerCase().includes(q) || p.workspace.toLowerCase().includes(q),
    );
    active = Math.max(
        0,
        matches.findIndex((p) => p.id === projectId),
    );
    renderList();
}

function renderList(): void {
    const several = new Set(projects.map((p) => p.workspace)).size > 1;
    const rows: HTMLElement[] = [];
    let lastWorkspace: string | null = null;
    matches.forEach((p, i) => {
        if (several && p.workspace !== lastWorkspace) {
            lastWorkspace = p.workspace;
            const head = document.createElement('li');
            head.className = 'combo-group';
            head.setAttribute('role', 'presentation');
            head.textContent = p.workspace;
            rows.push(head);
        }
        const li = document.createElement('li');
        li.id = `project-${p.id}`;
        li.className = 'combo-option';
        li.setAttribute('role', 'option');
        li.setAttribute('aria-selected', String(i === active));
        li.dataset.index = String(i);
        const name = document.createElement('span');
        name.textContent = p.name;
        li.append(name);
        if (p.key) {
            const key = document.createElement('span');
            key.className = 'combo-key';
            key.textContent = p.key;
            li.append(key);
        }
        // mousedown: before the input's blur closes the list
        li.addEventListener('mousedown', (e) => {
            e.preventDefault();
            choose(p.id);
        });
        rows.push(li);
    });
    if (!matches.length) {
        const empty = document.createElement('li');
        empty.className = 'combo-empty muted';
        empty.textContent = projects.length ? 'No project matches.' : 'No projects to create tasks in.';
        rows.push(empty);
    }
    projectList.replaceChildren(...rows);
    const current = matches[active];
    if (current) {
        projectInput.setAttribute('aria-activedescendant', `project-${current.id}`);
        document.getElementById(`project-${current.id}`)?.scrollIntoView({ block: 'nearest' });
    }
}

function choose(id: number): void {
    const changed = id !== projectId;
    projectId = id;
    closeList();
    renderProjectInput();
    if (changed) {
        boardTouched = false;
        typeId = null;
        priorityId = null;
        renderBoards(null);
        void loadConfig();
    }
    title.focus();
}

projectInput.addEventListener('focus', () => {
    projectInput.select();
    openList();
});
projectInput.addEventListener('input', () => {
    filterList(projectInput.value);
    if (projectList.hidden) openList();
});
projectInput.addEventListener('blur', () => {
    closeList();
    renderProjectInput();
});
projectInput.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        if (projectList.hidden) return openList();
        if (!matches.length) return;
        active = (active + (e.key === 'ArrowDown' ? 1 : matches.length - 1)) % matches.length;
        renderList();
    } else if (e.key === 'Enter' && !(e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        const p = matches[active];
        if (!projectList.hidden && p) choose(p.id);
    } else if (e.key === 'Escape' && !projectList.hidden) {
        // the list closes, not the window
        e.preventDefault();
        e.stopPropagation();
        closeList();
        renderProjectInput();
        projectInput.select();
    }
});

// ── board, priority, type ───────────────────────────────────────────────────

function renderBoards(preferred: number | null): void {
    const p = project();
    const boards = p?.boards ?? [];
    if (!boards.some((b) => b.id === boardId) || (!boardTouched && preferred !== null)) {
        boardId = boards.some((b) => b.id === preferred) ? preferred : (boards[0]?.id ?? null);
    }
    boardSelect.replaceChildren(
        ...boards.map((b) => {
            const o = document.createElement('option');
            o.value = String(b.id);
            o.textContent = b.name;
            return o;
        }),
    );
    if (boardId !== null) boardSelect.value = String(boardId);
    // one board: nothing to choose
    boardField.hidden = boards.length <= 1;
    fit();
}

boardSelect.addEventListener('change', () => {
    boardId = Number(boardSelect.value);
    boardTouched = true;
});

const PRIORITY_MARK: Record<string, string> = {
    highest: '⇈',
    high: '↑',
    medium: '=',
    low: '↓',
    lowest: '⇊',
};

function renderPriorities(): void {
    const list = config?.priorities ?? [];
    const chip = (id: number | null, name: string, slug: string, color: string | null): HTMLButtonElement => {
        const b = document.createElement('button');
        b.type = 'button';
        b.className = 'chip';
        b.setAttribute('role', 'radio');
        b.setAttribute('aria-checked', String(priorityId === id));
        b.tabIndex = priorityId === id ? 0 : -1;
        const mark = document.createElement('span');
        mark.className = 'chip-mark';
        mark.setAttribute('aria-hidden', 'true');
        mark.textContent = id === null ? '–' : (PRIORITY_MARK[slug] ?? '•');
        if (color) mark.style.color = color;
        const label = document.createElement('span');
        label.textContent = name;
        b.append(mark, label);
        b.addEventListener('click', () => {
            priorityId = id;
            renderPriorities();
            priorities.querySelector<HTMLButtonElement>('[aria-checked="true"]')?.focus();
        });
        return b;
    };
    if (priorityId !== null && !list.some((p) => p.id === priorityId)) priorityId = null;
    priorities.replaceChildren(chip(null, 'None', 'none', null), ...list.map((p) => chip(p.id, p.name, p.slug, p.color)));
}

// arrow keys move between the chips (one radio group)
priorities.addEventListener('keydown', (e) => {
    if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(e.key)) return;
    e.preventDefault();
    const chips = [...priorities.querySelectorAll<HTMLButtonElement>('.chip')];
    const at = chips.indexOf(document.activeElement as HTMLButtonElement);
    const next = chips[(at + (e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? chips.length - 1 : 1)) % chips.length];
    next?.click();
});

function renderTypes(): void {
    const types = config?.types ?? [];
    typeField.hidden = types.length === 0;
    if (typeId !== null && !types.some((t) => t.id === typeId)) typeId = null;
    typeSelect.replaceChildren(
        ...types.map((t) => {
            const o = document.createElement('option');
            o.value = String(t.id);
            o.textContent = t.name;
            return o;
        }),
    );
    if (typeId !== null) typeSelect.value = String(typeId);
    fit();
}

typeSelect.addEventListener('change', () => {
    typeId = Number(typeSelect.value);
});

async function loadConfig(): Promise<void> {
    const id = projectId;
    if (id === null) return;
    const seq = ++configSeq;
    config = null;
    renderPriorities();
    renderTypes();
    try {
        const c = await invoke<ConfigView>('task_config', { projectId: id });
        if (seq !== configSeq || id !== projectId) return;
        config = c;
        if (typeId === null) typeId = c.type_id;
        renderBoards(c.board_id);
        renderPriorities();
        renderTypes();
    } catch (e) {
        if (seq !== configSeq) return;
        // offline: no priorities / types to choose; the task can still be tried or go to the inbox
        const err = asError(e);
        if (err.kind !== 'offline') showError(err);
    }
}

// ── description ─────────────────────────────────────────────────────────────

function openDescription(focus: boolean): void {
    description.hidden = false;
    descToggle.setAttribute('aria-expanded', 'true');
    if (focus) description.focus();
    fit();
}

descToggle.addEventListener('click', () => {
    if (description.hidden) openDescription(true);
    else if (!description.value.trim()) {
        description.hidden = true;
        descToggle.setAttribute('aria-expanded', 'false');
        title.focus();
        fit();
    } else description.focus();
});

// ── loading ─────────────────────────────────────────────────────────────────

function applyProjects(next: Project[], selection: View['selection']): void {
    projects = next;
    if (!projects.some((p) => p.id === projectId)) {
        projectId = selection.project_id;
        boardTouched = false;
        typeId = null;
        renderBoards(selection.board_id);
        renderProjectInput();
        if (projectId !== null) void loadConfig();
    } else {
        renderProjectInput();
        renderBoards(null);
        if (!config) void loadConfig();
    }
    if (!projectList.hidden) filterList(projectInput.value);
    updateCreate();
}

function updateCreate(): void {
    createButton.disabled = sending || projectId === null;
}

async function refresh(): Promise<void> {
    // a new press after a created task: a fresh form (same project / board / type)
    if (!$('done').hidden) resetForm();
    let v: View;
    try {
        v = await invoke<View>('task_view');
    } catch (e) {
        setStatus(errorMessage(e), 'error');
        return;
    }
    view = v;
    $('create-keys').textContent = v.platform === 'macos' ? '⌘↩' : 'Ctrl+↩';
    $('where').textContent = v.server ? `in ${v.server}` : '';
    if (v.state !== 'ready') {
        $('blocked-title').textContent = v.state === 'no_server' ? 'Connect to Brainitive' : 'Sign in to Brainitive';
        $('blocked-text').textContent =
            v.state === 'no_server'
                ? 'Choose your Brainitive server in the main window first.'
                : `Sign in to ${v.server ?? 'your server'} to create tasks from anywhere.`;
        show('blocked');
        $('blocked-button').focus();
        return;
    }
    show('form');
    applyProjects(v.projects, v.selection);
    if (document.activeElement !== description && document.activeElement !== projectInput) title.focus();
    if (v.stale || !v.projects.length) await reload();
}

async function reload(): Promise<void> {
    try {
        const v = await invoke<View>('task_load');
        view = v;
        applyProjects(v.projects, v.selection);
    } catch (e) {
        const err = asError(e);
        if (err.kind === 'offline' || err.kind === 'server') {
            if (!projects.length) {
                projectInput.placeholder = 'Offline: projects can’t be loaded';
                setStatus('You’re offline. You can save this to your inbox and turn it into a task later.', 'muted');
                inboxButton.hidden = false;
            }
        } else showError(err);
        fit();
    }
}

// ── create ──────────────────────────────────────────────────────────────────

async function submit(): Promise<void> {
    if (sending || form.hidden) return;
    clearErrors();
    if (projectId === null) {
        showError({ kind: 'invalid', message: 'Pick a project.', fields: { project: 'Pick a project.' } });
        projectInput.focus();
        return;
    }
    if (!title.value.trim()) {
        showError({ kind: 'invalid', message: 'Give the task a title.', fields: { title: 'Give the task a title.' } });
        title.focus();
        return;
    }
    sending = true;
    busy(createButton, true);
    setStatus('Creating…');
    try {
        const created = await invoke<Created>('task_create', {
            input: {
                project_id: projectId,
                board_id: boardId,
                title: title.value,
                description: description.hidden ? '' : description.value,
                priority_id: priorityId,
                type_id: config?.types.length ? typeId : null,
            },
        });
        setStatus('');
        $('done-key').textContent = created.key ?? 'Task';
        $('done-name').textContent = created.title;
        ($('open') as HTMLButtonElement).hidden = !created.can_open;
        show('done');
        ($(created.can_open ? 'open' : 'again') as HTMLButtonElement).focus();
    } catch (e) {
        const err = asError(e);
        showError(err);
        if (err.kind === 'not_found') void reload();
        const first = Object.keys(err.fields ?? {})[0];
        if (first && first !== 'form') fieldInput[first]?.().focus();
    } finally {
        sending = false;
        busy(createButton, false);
        updateCreate();
    }
}

async function saveToInbox(): Promise<void> {
    if (!title.value.trim() && !description.value.trim()) {
        showError({ kind: 'invalid', message: 'Write something first.', fields: { title: 'Write something first.' } });
        return;
    }
    busy(inboxButton, true);
    try {
        const r = await invoke<CaptureResult>('task_save_to_inbox', {
            title: title.value,
            description: description.value,
        });
        clearErrors();
        setStatus(r.message, r.status === 'saved' ? 'ok' : 'muted');
        title.value = '';
        description.value = '';
        window.setTimeout(() => void invoke('task_close'), r.status === 'saved' ? 900 : 2400);
    } catch (e) {
        showError(asError(e));
    } finally {
        busy(inboxButton, false);
    }
}

function resetForm(): void {
    title.value = '';
    description.value = '';
    description.hidden = true;
    descToggle.setAttribute('aria-expanded', 'false');
    priorityId = null;
    renderPriorities();
    clearErrors();
    show('form');
}

form.addEventListener('submit', (e) => {
    e.preventDefault();
    void submit();
});
inboxButton.addEventListener('click', () => void saveToInbox());
$('again').addEventListener('click', () => {
    resetForm();
    title.focus();
});
$('open').addEventListener('click', () => {
    invoke('task_open').catch((e) => setStatus(asError(e).message, 'error'));
});
$('blocked-button').addEventListener('click', () => void invoke('task_open_app'));

document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
        e.preventDefault();
        void invoke('task_close');
    } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        if (!form.hidden) void submit();
    }
});

new ResizeObserver(() => fit()).observe(app);
renderPriorities();
onShown(() => void refresh());
void refresh();
