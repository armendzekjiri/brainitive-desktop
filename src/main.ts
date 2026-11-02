// The bundled screens: connect a server (BRAI-354), sign in with the browser (BRAI-355).
// Everything that talks to a server runs in Rust (src-tauri/src/flows.rs); this page only
// renders state and forwards clicks. Server-provided text is set with textContent only.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

type Phase = 'choose' | 'sign_in' | 'waiting' | 'connecting' | 'unreachable' | 'update_required' | 'in_app';

interface Notice {
    level: 'info' | 'error';
    message: string;
}

interface ServerSummary {
    origin: string;
    display_name: string;
    name: string;
    version: string | null;
    is_cloud?: boolean;
}

interface RecentEntry extends ServerSummary {
    last_used: number;
    is_current: boolean;
}

interface Problem {
    kind: string;
    title: string;
    message: string;
    fix: string;
    retryable: boolean;
    retry_at_ms: number | null;
    attempts: number;
}

interface UpdateRequired {
    server_name: string;
    origin: string;
    display_name: string;
    server_version: string | null;
    min_version: string;
    app_version: string;
    download_url: string;
}

// auto-updates (BRAI-373)
interface Updater {
    available: boolean;
    status:
        | { state: 'idle' | 'checking' | 'up_to_date' }
        | { state: 'downloading'; version: string; percent: number | null }
        | { state: 'ready' | 'installing'; version: string }
        | { state: 'failed'; message: string };
}

interface UiState {
    phase: Phase;
    notice: Notice | null;
    problem: Problem | null;
    update: UpdateRequired | null;
    current: ServerSummary | null;
    recent: RecentEntry[];
    app_version: string;
    cloud_origin: string;
    presets: { label: string; origin: string }[];
    updater: Updater;
}

interface ServerCheck {
    origin: string;
    entered: string;
    redirected: boolean;
    display_name: string;
    name: string;
    version: string | null;
    min_desktop_version: string | null;
    app_too_old: boolean;
}

interface UiError {
    kind: string;
    message: string;
}

const $ = <T extends HTMLElement = HTMLElement>(id: string): T => {
    const el = document.getElementById(id);
    if (!el) throw new Error(`#${id} missing`);
    return el as T;
};

const views = ['choose', 'confirm', 'sign_in', 'waiting', 'connecting', 'unreachable', 'update_required'] as const;
type View = (typeof views)[number];

let state: UiState | null = null;
let pendingCheck: ServerCheck | null = null;
let lastSignInUrl: string | null = null;
let countdown: number | undefined;

function errorMessage(e: unknown): string {
    if (e && typeof e === 'object' && 'message' in e) return String((e as UiError).message);
    return String(e);
}

function show(view: View): void {
    for (const v of views) $(`view-${v}`).hidden = v !== view;
}

function setText(selector: string, text: string): void {
    document.querySelectorAll<HTMLElement>(selector).forEach((el) => (el.textContent = text));
}

function showError(id: string, message: string | null): void {
    const el = $(id);
    el.textContent = message ?? '';
    el.hidden = !message;
}

function busy(button: HTMLButtonElement, on: boolean): void {
    button.disabled = on;
    button.setAttribute('aria-busy', String(on));
}

function renderNotice(notice: Notice | null): void {
    const box = $('notice');
    box.hidden = !notice;
    box.className = `notice ${notice?.level ?? ''}`;
    $('notice-text').textContent = notice?.message ?? '';
}

function renderRecent(s: UiState): void {
    const list = $('recent-list');
    list.replaceChildren();
    for (const r of s.recent) {
        const li = document.createElement('li');
        const open = document.createElement('button');
        open.type = 'button';
        open.className = 'recent-open';
        const name = document.createElement('strong');
        name.textContent = r.name;
        const origin = document.createElement('small');
        origin.textContent = r.is_current ? `${r.display_name} · current` : r.display_name;
        open.append(name, origin);
        open.addEventListener('click', () => void useServer(r.origin, open));

        const forget = document.createElement('button');
        forget.type = 'button';
        forget.className = 'link danger';
        forget.textContent = 'Forget';
        forget.title = `Sign out of ${r.display_name} and delete its data on this computer`;
        forget.addEventListener('click', () => void forgetServer(r, forget));

        li.append(open, forget);
        list.append(li);
    }
    $('recent').hidden = s.recent.length === 0;
}

function renderPresets(s: UiState): void {
    const list = $('preset-list');
    list.replaceChildren();
    for (const p of s.presets) {
        const b = document.createElement('button');
        b.type = 'button';
        b.className = 'chip';
        b.textContent = p.label;
        b.addEventListener('click', () => void check(p.origin, b));
        list.append(b);
    }
    $('presets').hidden = s.presets.length === 0;
}

function setOptional(id: string, text: string | null): void {
    const el = $(id);
    el.textContent = text ?? '';
    el.hidden = !text;
}

// "Trying again in 12 s": the server side retries by itself, this only shows when
function renderRetryLine(p: Problem | null): void {
    window.clearInterval(countdown);
    const line = $('problem-retry');
    if (p && !p.retryable) {
        line.textContent = 'Brainitive will not retry by itself: this needs a fix first.';
        return;
    }
    const at = p?.retry_at_ms;
    if (!at) {
        line.textContent = '';
        return;
    }
    const tick = (): void => {
        const left = Math.max(0, Math.ceil((at - Date.now()) / 1000));
        line.textContent = left > 0 ? `Trying again in ${left} s…` : 'Trying again…';
    };
    tick();
    countdown = window.setInterval(tick, 1000);
}

function renderProblem(p: Problem | null): void {
    if (p) $('problem-title').textContent = p.title;
    else
        $('problem-title').replaceChildren(
            Object.assign(document.createElement('span'), { className: 'current-name', textContent: state?.current?.name ?? 'The server' }),
            " can't be reached",
        );
    setOptional('problem-message', p?.message ?? null);
    setOptional('problem-fix', p?.fix ?? null);
    showError('retry-error', null);
    renderRetryLine(p);
}

function renderUpdate(u: UpdateRequired | null): void {
    if (!u) return;
    $('update-server').textContent = u.server_name;
    $('update-origin').textContent = u.display_name;
    $('update-app').textContent = u.app_version;
    $('update-min').textContent = `${u.min_version} or later`;
    $('update-server-version').textContent = u.server_version ?? 'unknown';
    $('update-url').textContent = u.download_url;
    showError('update-error', null);
}

let updaterReady = false;

/** The "Please update" screen with the auto-updater: "Check for update now", then "Restart to Update". */
function renderUpdater(up: Updater): void {
    const now = $<HTMLButtonElement>('update-now');
    const status = $('update-auto-status');
    now.hidden = !up.available;
    status.hidden = !up.available;
    if (!up.available) return;
    const s = up.status;
    updaterReady = s.state === 'ready';
    now.textContent = updaterReady ? 'Restart to Update' : 'Check for update now';
    now.classList.toggle('primary', updaterReady);
    now.disabled = ['checking', 'downloading', 'installing'].includes(s.state);
    status.textContent =
        s.state === 'checking'
            ? 'Checking for updates…'
            : s.state === 'downloading'
              ? `Downloading ${s.version}${s.percent !== null ? `: ${s.percent} %` : '…'}`
              : s.state === 'ready'
                ? `Version ${s.version} is ready. Restart to install it.`
                : s.state === 'installing'
                  ? `Installing ${s.version}…`
                  : s.state === 'up_to_date'
                    ? 'No newer version is published yet. Try again later, or download it by hand.'
                    : s.state === 'failed'
                      ? s.message
                      : '';
    status.hidden = !status.textContent;
}

async function refresh(): Promise<void> {
    state = await invoke<UiState>('get_state');
    $('app-version').textContent = state.app_version;
    renderNotice(state.notice);
    setText('.current-name', state.current?.name ?? 'your server');
    setText('.current-origin', state.current?.origin ?? '');
    renderRecent(state);
    renderPresets(state);
    if (state.phase !== 'unreachable') window.clearInterval(countdown);
    if (state.phase === 'unreachable') renderProblem(state.problem);
    if (state.phase === 'update_required') {
        renderUpdate(state.update);
        renderUpdater(state.updater);
    }

    switch (state.phase) {
        case 'choose':
            show(pendingCheck ? 'confirm' : 'choose');
            break;
        case 'in_app':
        case 'connecting':
            show('connecting');
            break;
        default:
            show(state.phase);
    }
}

async function check(input: string, button: HTMLButtonElement): Promise<void> {
    showError('check-error', null);
    busy(button, true);
    try {
        const result = await invoke<ServerCheck>('check_server', { input });
        pendingCheck = result;
        $('confirm-name').textContent = result.name;
        $('confirm-origin').textContent = result.origin;
        $('confirm-version').textContent = result.version ? `Brainitive ${result.version}` : 'Brainitive';
        // the server answered at another address than the one entered: say so before "Use"
        showError(
            'confirm-redirect',
            result.redirected
                ? `You entered ${result.entered}, but the server answers at ${result.origin}. Use it only if you trust that address.`
                : null,
        );
        showError(
            'confirm-too-old',
            result.app_too_old
                ? `This server needs Brainitive Desktop ${result.min_desktop_version} or later (this is ${state?.app_version}). Please update the app.`
                : null,
        );
        ($('confirm-use') as HTMLButtonElement).disabled = result.app_too_old;
        show('confirm');
    } catch (e) {
        showError('check-error', errorMessage(e));
    } finally {
        busy(button, false);
    }
}

async function useServer(origin: string, button?: HTMLButtonElement): Promise<void> {
    if (button) busy(button, true);
    try {
        await invoke('use_server', { origin });
        pendingCheck = null;
        await refresh();
    } catch (e) {
        pendingCheck = null;
        show('choose');
        showError('check-error', errorMessage(e));
    } finally {
        if (button) busy(button, false);
    }
}

// two clicks instead of window.confirm (not implemented by every webview)
async function forgetServer(r: RecentEntry, button: HTMLButtonElement): Promise<void> {
    if (button.dataset.armed !== '1') {
        button.dataset.armed = '1';
        button.textContent = 'Click again to forget';
        window.setTimeout(() => {
            button.dataset.armed = '';
            button.textContent = 'Forget';
        }, 4000);
        return;
    }
    busy(button, true);
    await invoke('forget_server', { origin: r.origin });
    await refresh();
}

async function signIn(button: HTMLButtonElement): Promise<void> {
    showError('sign-in-error', null);
    busy(button, true);
    try {
        lastSignInUrl = await invoke<string>('start_sign_in');
        await refresh();
    } catch (e) {
        showError('sign-in-error', errorMessage(e));
    } finally {
        busy(button, false);
    }
}

async function retry(errorId: string): Promise<void> {
    showError(errorId, null);
    try {
        await invoke('retry_connect');
    } catch (e) {
        showError(errorId, errorMessage(e));
    }
    await refresh();
}

function wire(): void {
    $('use-cloud').addEventListener('click', (ev) => {
        if (state) void check(state.cloud_origin, ev.currentTarget as HTMLButtonElement);
    });
    $('self-hosted').addEventListener('submit', (ev) => {
        ev.preventDefault();
        void check(($('server-url') as HTMLInputElement).value, $('check') as HTMLButtonElement);
    });
    $('confirm-back').addEventListener('click', () => {
        pendingCheck = null;
        show('choose');
    });
    $('confirm-use').addEventListener('click', (ev) => {
        if (pendingCheck) void useServer(pendingCheck.origin, ev.currentTarget as HTMLButtonElement);
    });
    $('sign-in').addEventListener('click', (ev) => void signIn(ev.currentTarget as HTMLButtonElement));
    $('sign-in-again').addEventListener('click', (ev) => void signIn(ev.currentTarget as HTMLButtonElement));
    $('cancel-sign-in').addEventListener('click', () => void invoke('cancel_sign_in').then(refresh));
    $('copy-link').addEventListener('click', () => {
        if (lastSignInUrl) void navigator.clipboard.writeText(lastSignInUrl);
    });
    $('retry').addEventListener('click', () => void retry('retry-error'));
    $('update-check').addEventListener('click', () => void retry('update-error'));
    $('update-now').addEventListener('click', () => {
        showError('update-error', null);
        // "Restart to Update" is the only thing that installs: the user's click
        invoke(updaterReady ? 'restart_to_update' : 'check_for_update')
            .catch((e) => showError('update-error', errorMessage(e)))
            .finally(() => void refresh());
    });
    $('update-download').addEventListener('click', () => {
        invoke('open_download_page').catch((e) => showError('update-error', errorMessage(e)));
    });
    // BRAI-366: the bundled offline window (this computer's copy); the web app's own offline
    // mode (service worker, opt-in on the web) stays one click away
    $('open-offline').addEventListener('click', () => {
        invoke('open_offline').catch((e) => showError('retry-error', errorMessage(e)));
    });
    $('open-web-offline').addEventListener('click', () => {
        invoke('open_web_offline').catch((e) => showError('retry-error', errorMessage(e)));
    });
    document.querySelectorAll<HTMLButtonElement>('.switch').forEach((b) =>
        b.addEventListener('click', () => {
            pendingCheck = null;
            void invoke('switch_server').then(refresh);
        }),
    );
    $('notice-close').addEventListener('click', () => void invoke('dismiss_notice').then(refresh));
}

// script errors of the bundled screens go to the local log file (never to a server)
function reportError(message: string, source: string): void {
    invoke('log_client_error', { message, source }).catch(() => undefined);
}
window.addEventListener('error', (ev) => reportError(ev.message, `${ev.filename}:${ev.lineno}`));
window.addEventListener('unhandledrejection', (ev) => reportError(`unhandled rejection: ${errorMessage(ev.reason)}`, 'promise'));

wire();
void listen('desktop://changed', () => void refresh());
void refresh();
