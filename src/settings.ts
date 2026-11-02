// The Settings window (menu → Settings…, ⌘, / Ctrl+,). Desktop preferences only; the account's
// own settings stay in the web app ("Account & notifications…").

import { invoke } from '@tauri-apps/api/core';
import { $, errorMessage, onShown } from './native';

interface Prefs {
    app_version: string;
    platform: 'macos' | 'windows' | 'linux';
    server: string | null;
    signed_in: boolean;
    zoom_percent: number;
    capture_shortcut: string;
    switcher_shortcut: string;
    voice_shortcut: string;
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
}

// auto-updates (BRAI-373)
type UpdateStatus =
    | { state: 'idle' | 'checking' | 'up_to_date' }
    | { state: 'downloading'; version: string; percent: number | null }
    | { state: 'ready' | 'installing'; version: string }
    | { state: 'failed'; message: string };

interface Updates {
    available: boolean;
    unavailable_reason: string | null;
    auto_check: boolean;
    channel: 'stable' | 'beta';
    last_check: number | null;
    current_version: string;
    status: UpdateStatus;
}

type Toggle = 'keep_in_tray' | 'ask_where_to_save' | 'launch_at_login' | 'start_hidden' | 'voice_enabled';

type Which = 'capture' | 'switcher' | 'voice';
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
        const label = { capture: p.capture_shortcut, switcher: p.switcher_shortcut, voice: p.voice_shortcut }[which];
        const current = row.querySelector<HTMLElement>('.current');
        if (current) current.textContent = label || 'Off';
    });
    renderVoice(p);
    renderUpdates(p.updates);
}

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
    action.textContent = updateReady ? 'Restart to Update' : 'Check now';
    action.disabled = !u.available || ['checking', 'downloading', 'installing'].includes(s.state);
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

onShown(() => void refresh());
void refresh();
