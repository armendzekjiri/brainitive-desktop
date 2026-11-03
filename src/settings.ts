// The Settings window (menu → Settings…, ⌘, / Ctrl+,). Desktop preferences only; the account's
// own settings stay in the web app ("Account & notifications…"). Shortcuts: the Brainitive key and
// its letters (BRAI-440), then the optional direct shortcuts.

import { invoke } from '@tauri-apps/api/core';
import { $, errorMessage, onShown } from './native';
import { animateWindow } from './motion';

// the window's content rises in each time it is shown (motion.css)
animateWindow();

interface Prefs {
    app_version: string;
    platform: 'macos' | 'windows' | 'linux';
    server: string | null;
    signed_in: boolean;
    zoom_percent: number;
    leader_shortcut: string;
    leader_letters: Letter[];
    capture_shortcut: string;
    switcher_shortcut: string;
    voice_shortcut: string;
    task_shortcut: string;
    // what Default gives New task here ('' on Linux: off)
    task_default: string;
    voice_enabled: boolean;
    voice_language: string;
    voice_languages: { code: string; name: string }[];
    voice_model: { name: string; size_mb: number; ready: boolean; downloading: number | null };
    keep_in_tray: boolean;
    ask_where_to_save: boolean;
    launch_at_login: boolean;
    start_hidden: boolean;
    shortcut_problem: string | null;
    updates: Updates;
    // a section to show once ("servers": Manage Servers…)
    focus: string | null;
}

// Settings → Servers (ids and names only; switching takes the id)
interface ServerRow {
    id: string;
    name: string;
    default_name: string;
    host: string;
    current: boolean;
    signed_in: boolean;
    last_used: number;
    shortcut: string | null;
}

interface ServersView {
    servers: ServerRow[];
    max: number;
}

// the Brainitive key's letters (BRAI-440)
type LetterId = 'capture' | 'task' | 'switcher' | 'voice' | 'main';

interface Letter {
    id: LetterId;
    name: string;
    letter: string;
    default: string;
}

// auto-updates (BRAI-373)
type UpdateStatus =
    | { state: 'idle' | 'checking' | 'up_to_date' }
    | { state: 'downloading'; version: string; percent: number | null }
    | { state: 'ready' | 'installing'; version: string }
    | { state: 'failed'; message: string };

interface Updates {
    available: boolean;
    /** The Microsoft Store build (BRAI-442): the Store updates the app. */
    store: boolean;
    unavailable_reason: string | null;
    auto_check: boolean;
    channel: 'stable' | 'beta';
    last_check: number | null;
    current_version: string;
    status: UpdateStatus;
}

type Toggle = 'keep_in_tray' | 'ask_where_to_save' | 'launch_at_login' | 'start_hidden' | 'voice_enabled';

type Which = 'leader' | 'capture' | 'switcher' | 'voice' | 'task';
type Choice = { mode: 'off' } | { mode: 'default' } | { mode: 'keys'; keys: string };

let recording: { which: Which; button: HTMLButtonElement } | null = null;

function render(p: Prefs): void {
    $('version').textContent = p.app_version;
    document.querySelectorAll('.tray-name').forEach((el) => (el.textContent = p.platform === 'macos' ? 'menu bar' : 'tray'));
    document.querySelectorAll<HTMLInputElement>('[data-toggle]').forEach((input) => {
        const key = input.dataset.toggle as Toggle;
        input.checked = Boolean(p[key]);
    });
    ($('start_hidden') as HTMLInputElement).disabled = !p.launch_at_login;
    const problem = $('shortcut-problem');
    problem.textContent = recording ? '' : (p.shortcut_problem ?? '');
    problem.hidden = recording !== null || !p.shortcut_problem;
    document.querySelectorAll('.mod').forEach((el) => (el.textContent = p.platform === 'macos' ? '⌘' : 'Ctrl+'));
    $('zoom').textContent = `${p.zoom_percent} %`;
    $('server').textContent = p.server ?? 'Not connected';
    $('server-state').textContent = p.server ? (p.signed_in ? 'Signed in' : 'Not signed in') : '';
    ($('open-account') as HTMLButtonElement).disabled = !p.signed_in;
    document.querySelectorAll<HTMLElement>('[data-shortcut]').forEach((row) => {
        const which = row.dataset.shortcut as Which;
        const label = {
            leader: p.leader_shortcut,
            capture: p.capture_shortcut,
            switcher: p.switcher_shortcut,
            voice: p.voice_shortcut,
            task: p.task_shortcut,
        }[which];
        const current = row.querySelector<HTMLElement>('.current');
        if (current) current.textContent = label || 'Off';
    });
    // New task has no default on Linux (Ctrl+Alt+T opens the terminal): Default = Off there
    const taskReset = document.querySelector<HTMLButtonElement>('[data-shortcut="task"] .reset');
    if (taskReset) taskReset.title = p.task_default ? `Back to ${p.task_default}` : 'Off by default here (Ctrl+Alt+T opens the terminal)';
    renderLetters(p);
    renderVoice(p);
    renderUpdates(p.updates);
    $('servers-keys').textContent = p.platform === 'macos' ? '⌃⌘1…9' : 'Alt+1…9';
    if (p.focus === 'servers') $('servers-section').scrollIntoView({ block: 'start' });
}

// ── Settings → Servers ─────────────────────────────────────────────────────────

function serversError(message: string | null): void {
    const el = $('servers-error');
    el.textContent = message ?? '';
    el.hidden = !message;
}

function lastUsed(seconds: number): string {
    if (!seconds) return 'not used yet';
    const d = new Date(seconds * 1000);
    const today = new Date();
    return d.toDateString() === today.toDateString()
        ? `used today, ${d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`
        : `used ${d.toLocaleDateString()}`;
}

function button(text: string, onClick: (b: HTMLButtonElement) => void, title?: string): HTMLButtonElement {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = text;
    if (title) b.title = title;
    b.addEventListener('click', () => onClick(b));
    return b;
}

async function serverCall(command: string, args: Record<string, unknown>): Promise<void> {
    serversError(null);
    try {
        const view = await invoke<ServersView | null>(command, args);
        if (view) renderServers(view);
        else await refreshServers();
    } catch (e) {
        serversError(errorMessage(e));
        await refreshServers();
    }
}

function renderServers(v: ServersView): void {
    const list = $('servers');
    // a name being typed keeps what the user typed
    const editing = document.activeElement instanceof HTMLInputElement && document.activeElement.classList.contains('server-name')
        ? document.activeElement.dataset.id
        : undefined;
    if (editing) return;
    list.replaceChildren(
        ...v.servers.map((s, i) => {
            const li = document.createElement('li');
            li.className = s.current ? 'server current' : 'server';

            const name = document.createElement('input');
            name.className = 'server-name';
            name.type = 'text';
            name.maxLength = 60;
            name.spellcheck = false;
            name.value = s.name;
            name.placeholder = s.default_name;
            name.dataset.id = s.id;
            name.setAttribute('aria-label', `Name of ${s.host}`);
            const save = (): void => {
                if (name.value.trim() === s.name) return;
                void serverCall('settings_rename_server', { id: s.id, name: name.value });
            };
            name.addEventListener('keydown', (e) => {
                if (e.key === 'Enter') name.blur();
                if (e.key === 'Escape') {
                    e.stopPropagation();
                    name.value = s.name;
                    name.blur();
                }
            });
            name.addEventListener('blur', save);

            const order = document.createElement('span');
            order.className = 'order';
            const up = button('↑', () => void serverCall('settings_move_server', { id: s.id, direction: 'up' }), 'Move up');
            const down = button('↓', () => void serverCall('settings_move_server', { id: s.id, direction: 'down' }), 'Move down');
            up.disabled = i === 0;
            down.disabled = i === v.servers.length - 1;
            order.append(up, down);

            const details = document.createElement('span');
            details.className = 'details';
            const signed = document.createElement('span');
            signed.className = s.signed_in ? 'on' : '';
            signed.textContent = s.signed_in ? 'Signed in' : 'Signed out';
            details.append(
                `${s.host} · `,
                signed,
                ` · ${lastUsed(s.last_used)}${s.shortcut ? ` · ${s.shortcut}` : ''}${s.current ? ' · current' : ''}`,
            );

            const actions = document.createElement('span');
            actions.className = 'actions';
            const switchTo = button(s.current ? 'Current' : 'Switch to', () => void serverCall('settings_switch_server', { id: s.id }));
            switchTo.disabled = s.current;
            const signOut = button('Sign out', () => void serverCall('settings_sign_out_server', { id: s.id }), 'Sign this computer out of the server; it stays in the list');
            signOut.disabled = !s.signed_in;
            // two clicks instead of window.confirm
            const forget = button('Forget', (b) => {
                if (b.dataset.armed !== '1') {
                    b.dataset.armed = '1';
                    b.textContent = 'Click again to forget';
                    window.setTimeout(() => {
                        b.dataset.armed = '';
                        b.textContent = 'Forget';
                    }, 4000);
                    return;
                }
                b.disabled = true;
                void serverCall('settings_forget_server', { id: s.id });
            }, 'Sign out, delete its data on this computer and remove it from the list');
            forget.className = 'danger';
            actions.append(switchTo, signOut, forget);

            li.append(name, order, details, actions);
            return li;
        }),
    );
    $('servers-count').textContent = v.servers.length ? `${v.servers.length} of ${v.max} servers` : 'No server yet.';
    ($('server-add') as HTMLButtonElement).disabled = v.servers.length >= v.max;
}

async function refreshServers(): Promise<void> {
    try {
        renderServers(await invoke<ServersView>('settings_servers'));
    } catch (e) {
        serversError(errorMessage(e));
    }
}

$('server-add').addEventListener('click', () => void invoke('settings_add_server').catch((e) => serversError(errorMessage(e))));

function renderLetters(p: Prefs): void {
    document.querySelectorAll('.leader-label').forEach((el) => (el.textContent = p.leader_shortcut || 'Brainitive key'));
    for (const l of p.leader_letters) {
        const input = document.querySelector<HTMLInputElement>(`[data-letter="${l.id}"] input.letter`);
        // a field being typed in keeps what the user typed (its error says why it was not taken)
        if (input && document.activeElement !== input) input.value = l.letter;
        const reset = document.querySelector<HTMLButtonElement>(`[data-letter="${l.id}"] .reset`);
        if (reset) reset.disabled = l.letter === l.default;
        if (l.id === 'voice') document.querySelectorAll('.voice-letter').forEach((el) => (el.textContent = l.letter));
    }
    // without the Brainitive key the letters do nothing
    document.querySelectorAll<HTMLElement>('.letter-row').forEach((row) => row.classList.toggle('muted', !p.leader_shortcut));
}

function letterError(id: LetterId, message: string | null): void {
    const el = document.querySelector<HTMLElement>(`.letter-error[data-for="${id}"]`);
    if (!el) return;
    el.textContent = message ?? '';
    el.hidden = !message;
}

async function setLetter(id: LetterId, letter: string | null): Promise<boolean> {
    letterError(id, null);
    try {
        render(await invoke<Prefs>('settings_set_leader_letter', { action: id, letter }));
        return true;
    } catch (e) {
        letterError(id, errorMessage(e));
        return false;
    }
}

document.querySelectorAll<HTMLElement>('[data-letter]').forEach((row) => {
    const id = row.dataset.letter as LetterId;
    const input = row.querySelector<HTMLInputElement>('input.letter');
    if (!input) return;
    input.addEventListener('focus', () => input.select());
    input.addEventListener('input', () => {
        const value = input.value.trim();
        if (!value) return;
        // taken or not, the next key replaces it
        void setLetter(id, value).then(() => input.select());
    });
    // a letter that was not taken goes back to the one in use (the error stays to say why)
    input.addEventListener('blur', () => void refresh());
    row.querySelector('.reset')?.addEventListener('click', () => void setLetter(id, null));
});

let updatePoll: number | undefined;
let updateReady = false;

function renderUpdates(u: Updates): void {
    const off = $('updates-off');
    off.textContent = u.unavailable_reason ?? '';
    off.hidden = u.available;
    const auto = $<HTMLInputElement>('auto_update_check');
    const beta = $<HTMLInputElement>('update_beta');
    auto.checked = u.auto_check;
    beta.checked = u.channel === 'beta';
    auto.disabled = beta.disabled = !u.available;
    // the Microsoft Store build: no channel, no automatic check of our own; the button opens the Store
    $('update-auto-row').hidden = $('update-beta-row').hidden = u.store;
    $('update-version').textContent = u.current_version;
    const s = u.status;
    const checked = u.last_check ? `Last checked ${new Date(u.last_check * 1000).toLocaleString()}.` : 'Not checked yet.';
    const text: Record<UpdateStatus['state'], string> = {
        idle: checked,
        checking: 'Checking for updates…',
        up_to_date: `This is the newest version. ${checked}`,
        downloading: 'version' in s ? `Downloading ${s.version}${'percent' in s && s.percent !== null ? `: ${s.percent} %` : '…'}` : '',
        ready: 'version' in s ? `Version ${s.version} is ready. It is installed when you restart.` : '',
        installing: 'version' in s ? `Installing ${s.version}…` : '',
        failed: 'message' in s ? s.message : '',
    };
    $('update-status').textContent = u.available ? text[s.state] : '';
    const action = $<HTMLButtonElement>('update-action');
    updateReady = u.available && s.state === 'ready';
    action.textContent = u.store ? 'Open Microsoft Store' : updateReady ? 'Restart to Update' : 'Check now';
    action.disabled = !u.store && (!u.available || ['checking', 'downloading', 'installing'].includes(s.state));
    // a check or download runs in the background: follow it
    window.clearTimeout(updatePoll);
    if (['checking', 'downloading', 'installing'].includes(s.state)) updatePoll = window.setTimeout(() => void refresh(), 1000);
}

function updateError(message: string | null): void {
    const el = $('update-error');
    el.textContent = message ?? '';
    el.hidden = !message;
}

function renderVoice(p: Prefs): void {
    const select = $<HTMLSelectElement>('voice_language');
    if (select.options.length !== p.voice_languages.length) {
        select.replaceChildren(
            ...p.voice_languages.map((l) => {
                const option = document.createElement('option');
                option.value = l.code;
                option.textContent = l.name;
                return option;
            }),
        );
    }
    select.value = p.voice_language;
    const m = p.voice_model;
    $('voice-model').textContent =
        m.downloading !== null
            ? `${m.name}: downloading, ${m.downloading} %`
            : m.ready
              ? `${m.name}: on this computer.`
              : `${m.name}: not downloaded. It is downloaded the first time you use voice capture.`;
    ($('voice-remove') as HTMLButtonElement).disabled = !m.ready && m.downloading === null;
}

function shortcutError(which: Which, message: string | null): void {
    const el = document.querySelector<HTMLElement>(`.shortcut-error[data-for="${which}"]`);
    if (!el) return;
    el.textContent = message ?? '';
    el.hidden = !message;
}

async function setShortcut(which: Which, choice: Choice): Promise<void> {
    shortcutError(which, null);
    try {
        render(await invoke<Prefs>('settings_set_shortcut', { which, choice }));
    } catch (e) {
        shortcutError(which, errorMessage(e));
    }
}

function stopRecording(): void {
    if (!recording) return;
    recording.button.classList.remove('recording');
    recording.button.textContent = 'Change';
    recording = null;
    void invoke('settings_record_shortcut', { active: false });
}

const MODIFIER_CODES = /^(Shift|Control|Alt|Meta|OS)(Left|Right)?$|^CapsLock$|^Fn$/;

/** KeyboardEvent → `Control+Alt+Shift+Super+KeyK` (the global-hotkey syntax). */
function accelerator(e: KeyboardEvent): string | null {
    if (MODIFIER_CODES.test(e.code) || !e.code) return null;
    const parts: string[] = [];
    if (e.ctrlKey) parts.push('Control');
    if (e.altKey) parts.push('Alt');
    if (e.shiftKey) parts.push('Shift');
    if (e.metaKey) parts.push('Super');
    parts.push(e.code);
    return parts.join('+');
}

document.querySelectorAll<HTMLElement>('[data-shortcut]').forEach((row) => {
    const which = row.dataset.shortcut as Which;
    const record = row.querySelector<HTMLButtonElement>('.record');
    record?.addEventListener('click', () => {
        if (recording) {
            stopRecording();
            return;
        }
        recording = { which, button: record };
        record.classList.add('recording');
        record.textContent = 'Press the keys…';
        shortcutError(which, null);
        void invoke('settings_record_shortcut', { active: true });
    });
    row.querySelector('.reset')?.addEventListener('click', () => void setShortcut(which, { mode: 'default' }));
    row.querySelector('.off')?.addEventListener('click', () => void setShortcut(which, { mode: 'off' }));
});

window.addEventListener('blur', stopRecording);

document.querySelectorAll<HTMLInputElement>('[data-toggle]').forEach((input) => {
    input.addEventListener('change', () => {
        const key = input.dataset.toggle as Toggle;
        const error = $('general-error');
        error.hidden = true;
        invoke<Prefs>('settings_set_toggle', { key, value: input.checked })
            .then(render)
            .catch((e) => {
                error.textContent = errorMessage(e);
                error.hidden = false;
                void refresh();
            });
    });
});

async function refresh(): Promise<void> {
    render(await invoke<Prefs>('settings_get'));
}

$('voice_language').addEventListener('change', (e) => {
    const language = (e.target as HTMLSelectElement).value;
    invoke<Prefs>('settings_set_voice_language', { language })
        .then(render)
        .catch((err) => voiceError(errorMessage(err)));
});
$('voice-remove').addEventListener('click', () => {
    voiceError(null);
    invoke<Prefs>('settings_remove_voice_model')
        .then(render)
        .catch((err) => voiceError(errorMessage(err)));
});

function voiceError(message: string | null): void {
    const el = $('voice-error');
    el.textContent = message ?? '';
    el.hidden = !message;
}

$('auto_update_check').addEventListener('change', (e) => {
    updateError(null);
    invoke<Prefs>('settings_set_toggle', { key: 'auto_update_check', value: (e.target as HTMLInputElement).checked })
        .then(render)
        .catch((err) => updateError(errorMessage(err)));
});
$('update_beta').addEventListener('change', (e) => {
    updateError(null);
    const channel = (e.target as HTMLInputElement).checked ? 'beta' : 'stable';
    invoke<Prefs>('settings_set_update_channel', { channel })
        .then(render)
        .catch((err) => updateError(errorMessage(err)));
});
$('update-action').addEventListener('click', () => {
    updateError(null);
    // only this click installs: the app restarts right after
    const call = updateReady ? invoke('settings_restart_to_update') : invoke<Prefs>('settings_check_for_updates');
    call.then(() => refresh()).catch((err) => updateError(errorMessage(err)));
});

$('zoom-reset').addEventListener('click', () => void invoke<Prefs>('settings_reset_zoom').then(render));
$('open-account').addEventListener('click', () => void invoke('settings_open_account'));
document.addEventListener('keydown', (e) => {
    if (recording) {
        e.preventDefault();
        if (e.key === 'Escape') {
            stopRecording();
            return;
        }
        const keys = accelerator(e);
        if (!keys) return;
        const which = recording.which;
        stopRecording();
        void setShortcut(which, { mode: 'keys', keys });
        return;
    }
    if (e.key === 'Escape') void invoke('settings_close');
});

// the server list reads the keychain (signed in?): only when the window comes up, and after a change
onShown(() => {
    void refresh();
    void refreshServers();
});
void refresh();
void refreshServers();
