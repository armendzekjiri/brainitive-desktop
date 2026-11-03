// The quick-capture window (BRAI-359): opened by the global shortcut (⇧⌘Space / Ctrl+Shift+Space),
// the tray or the menu, and by voice capture with the transcribed text (BRAI-439). The text goes to
// Rust (`capture_submit`), which posts it to the server's private inbox with the app token, or keeps
// it on this computer while offline.

import { invoke } from '@tauri-apps/api/core';
import { $, busy, errorMessage, onShown } from './native';
import { animateWindow } from './motion';

// the window's content rises in each time it is shown (motion.css)
animateWindow();

interface Context {
    server: string | null;
    signed_in: boolean;
    queued: number;
    shortcut: string;
    platform: 'macos' | 'windows' | 'linux';
    prefill: string | null;
}

interface Result {
    status: 'saved' | 'queued';
    message: string;
}

const text = $<HTMLTextAreaElement>('text');
const url = $<HTMLInputElement>('url');
const type = $<HTMLSelectElement>('type');
const save = $<HTMLButtonElement>('save');
const status = $('status');
let closeTimer: number | undefined;

function setStatus(message: string, kind: 'ok' | 'error' | 'muted' = 'muted'): void {
    status.textContent = message;
    status.className = `status ${kind}`;
}

async function refresh(): Promise<void> {
    window.clearTimeout(closeTimer);
    text.focus();
    text.select();
    try {
        const ctx = await invoke<Context>('capture_context');
        if (ctx.prefill) {
            // voice capture: added to what is already typed, never replacing it
            const before = text.value.trimEnd();
            text.value = before ? `${before} ${ctx.prefill}` : ctx.prefill;
            text.setSelectionRange(text.value.length, text.value.length);
        }
        $('save-keys').textContent = ctx.platform === 'macos' ? '⌘↩' : 'Ctrl+↩';
        $('where').textContent = ctx.server ? `to ${ctx.server}` : '';
        if (!ctx.server) setStatus('Connect Brainitive to a server first.', 'error');
        else if (!ctx.signed_in) setStatus(`Sign in to ${ctx.server} first.`, 'error');
        else if (ctx.queued > 0)
            setStatus(`${ctx.queued} capture${ctx.queued === 1 ? '' : 's'} waiting to be sent.`);
        else setStatus('');
        save.disabled = !ctx.server || !ctx.signed_in;
    } catch (e) {
        setStatus(errorMessage(e), 'error');
    }
}

async function submit(): Promise<void> {
    if (save.disabled) return;
    busy(save, true);
    try {
        const result = await invoke<Result>('capture_submit', {
            text: text.value,
            url: url.value || null,
            intendedType: type.value || null,
        });
        setStatus(result.message, result.status === 'saved' ? 'ok' : 'muted');
        text.value = '';
        url.value = '';
        type.value = '';
        closeTimer = window.setTimeout(() => void invoke('capture_close'), result.status === 'saved' ? 700 : 2200);
    } catch (e) {
        setStatus(errorMessage(e), 'error');
    } finally {
        busy(save, false);
    }
}

// voice capture (BRAI-439): the text comes back into this window
$('mic').addEventListener('click', () => {
    invoke('capture_voice').catch((e) => setStatus(errorMessage(e), 'error'));
});

$('form').addEventListener('submit', (e) => {
    e.preventDefault();
    void submit();
});
document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
        e.preventDefault();
        void invoke('capture_close');
    } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        void submit();
    }
});

onShown(() => void refresh());
void refresh();
