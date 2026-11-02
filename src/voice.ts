// The voice-capture indicator (BRAI-439): opened by the voice shortcut (⇧⌘V / Ctrl+Shift+V, held to
// talk), the tray, the menu or quick capture's microphone button. It only shows what Rust tells it
// (`voice://view`, `voice://level`) and asks Rust to start / stop / cancel: the microphone, the audio
// and the transcription never reach this page.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { $, errorMessage, onShown } from './native';

type Screen =
    | { screen: 'intro' }
    | { screen: 'permission' }
    | { screen: 'denied'; message: string }
    | { screen: 'downloading'; done: number; total: number }
    | { screen: 'download_failed'; message: string }
    | { screen: 'ready' }
    | { screen: 'recording'; elapsed_ms: number; max_ms: number; hold: boolean }
    | { screen: 'transcribing' }
    | { screen: 'error'; message: string };

type View = Screen & {
    shortcut: string;
    platform: 'macos' | 'windows' | 'linux';
    model_name: string;
    model_mb: number;
    can_open_settings: boolean;
};

interface Level {
    level: number;
    elapsed_ms: number;
}

let current: View | null = null;

function clock(ms: number): string {
    const s = Math.floor(ms / 1000);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

function mb(bytes: number): string {
    return String(Math.round(bytes / 1_000_000));
}

function render(v: View): void {
    current = v;
    document.querySelectorAll<HTMLElement>('[data-screen]').forEach((s) => (s.hidden = s.dataset.screen !== v.screen));
    document.querySelectorAll('.shortcut').forEach((el) => (el.textContent = v.shortcut || 'the shortcut'));
    document.querySelectorAll('.model-mb').forEach((el) => (el.textContent = String(v.model_mb)));
    switch (v.screen) {
        case 'denied':
            $('denied-message').textContent = v.message;
            $('privacy').hidden = !v.can_open_settings;
            break;
        case 'downloading': {
            const percent = v.total > 0 ? Math.floor((v.done * 100) / v.total) : 0;
            $<HTMLProgressElement>('download-bar').value = percent;
            $('download-text').textContent = `${percent} % (${mb(v.done)} of ${mb(v.total)} MB)`;
            break;
        }
        case 'download_failed':
            $('download-error').textContent = v.message;
            break;
        case 'recording': {
            $('timer').textContent = clock(v.elapsed_ms);
            $('max').textContent = clock(v.max_ms);
            const enter = '↩';
            $('recording-hint').textContent = v.hold
                ? `Let go of ${v.shortcut || 'the shortcut'} to finish`
                : `${enter}${v.shortcut ? ` or ${v.shortcut}` : ''} to finish`;
            setLevel(0);
            break;
        }
        case 'error':
            $('error-message').textContent = v.message;
            break;
        default:
            break;
    }
    const primary = document.querySelector<HTMLButtonElement>(`[data-screen="${v.screen}"] .primary`);
    primary?.focus();
}

function setLevel(level: number): void {
    const percent = Math.round(Math.max(0, Math.min(1, level)) * 100);
    $('meter-fill').style.width = `${percent}%`;
    $('meter').setAttribute('aria-valuenow', String(percent));
}

async function refresh(): Promise<void> {
    try {
        render(await invoke<View>('voice_view'));
    } catch (e) {
        render({ screen: 'error', message: errorMessage(e) } as View);
    }
}

const COMMANDS: Record<string, string> = {
    close: 'voice_close',
    'turn-on': 'voice_turn_on',
    privacy: 'voice_open_privacy',
    download: 'voice_download',
    start: 'voice_start',
    stop: 'voice_stop',
    cancel: 'voice_cancel',
};

document.querySelectorAll<HTMLButtonElement>('[data-action]').forEach((button) => {
    button.addEventListener('click', () => {
        const command = COMMANDS[button.dataset.action ?? ''];
        if (command) invoke(command).catch((e) => render({ screen: 'error', message: errorMessage(e) } as View));
    });
});

document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
        e.preventDefault();
        void invoke('voice_cancel');
    } else if (e.key === 'Enter' && !e.repeat && (current?.screen === 'recording' || current?.screen === 'ready')) {
        e.preventDefault();
        void invoke(current.screen === 'recording' ? 'voice_stop' : 'voice_start');
    }
});

void listen<View>('voice://view', (e) => render(e.payload));
void listen<Level>('voice://level', (e) => {
    if (current?.screen !== 'recording') return;
    setLevel(e.payload.level);
    $('timer').textContent = clock(e.payload.elapsed_ms);
});

onShown(() => void refresh());
void refresh();
