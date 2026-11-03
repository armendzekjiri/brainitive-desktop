// The Brainitive key overlay (BRAI-440): opened by the Brainitive key (⌃⌘K / Ctrl+Alt+K), lists the
// letters and reports the next key pressed here. Rust decides what runs (`leader_press`); no letter
// is registered globally. Voice capture's letter is held to talk: its key-up goes to
// `leader_release` (or, once the voice indicator has the focus, to that window).

import { invoke } from '@tauri-apps/api/core';
import { $, errorMessage, onShown } from './native';
import { animateWindow } from './motion';

// the window's content rises in each time it is shown (motion.css)
animateWindow();

interface Action {
    id: 'capture' | 'task' | 'switcher' | 'voice' | 'main';
    name: string;
    letter: string;
    enabled: boolean;
    hint: string | null;
}

interface View {
    leader: string;
    platform: 'macos' | 'windows' | 'linux';
    actions: Action[];
}

const list = $('actions');
const status = $('status');
let view: View | null = null;
/** The voice letter that is held down (its key-up ends the recording). */
let held: { letter: string; code: string } | null = null;

function setStatus(message: string, kind: 'muted' | 'error' = 'muted'): void {
    status.textContent = message;
    status.className = `status ${kind}`;
}

function render(v: View): void {
    view = v;
    const key = $('leader-key');
    key.textContent = v.leader;
    key.hidden = !v.leader;
    list.replaceChildren(
        ...v.actions.map((a) => {
            const li = document.createElement('li');
            li.dataset.id = a.id;
            li.classList.toggle('off', !a.enabled);
            const letter = document.createElement('kbd');
            letter.textContent = a.letter;
            const name = document.createElement('span');
            name.textContent = a.id === 'voice' ? `${a.name} (hold)` : a.name;
            li.append(letter, name);
            if (a.hint) {
                const hint = document.createElement('small');
                hint.className = 'hint muted';
                hint.textContent = a.hint;
                li.append(hint);
            }
            return li;
        }),
    );
}

async function refresh(): Promise<void> {
    held = null;
    setStatus('Press a letter.');
    try {
        render(await invoke<View>('leader_view'));
    } catch (e) {
        setStatus(errorMessage(e), 'error');
    }
}

/** The letter of a key: what the layout types (Y / Z on QWERTZ), else the key's position. */
function letterOf(e: KeyboardEvent): string | null {
    if (e.key.length === 1 && /^[a-z]$/i.test(e.key)) return e.key.toUpperCase();
    const m = /^Key([A-Z])$/.exec(e.code);
    return m?.[1] ?? null;
}

function mark(letter: string): void {
    list.querySelectorAll('li').forEach((li) => {
        const a = view?.actions.find((x) => x.id === li.dataset.id);
        li.classList.toggle('pressed', a?.letter === letter);
    });
}

document.addEventListener('keydown', (e) => {
    // every key is ours while the overlay is open (no beep, no focus moves)
    e.preventDefault();
    if (e.repeat) return;
    if (e.key === 'Escape') {
        void invoke('leader_close');
        return;
    }
    const letter = letterOf(e);
    if (!letter) {
        // a modifier on its own (⌃⌘ still held or pressed again): keep waiting
        return;
    }
    mark(letter);
    const voice = view?.actions.find((a) => a.id === 'voice');
    if (voice?.enabled && voice.letter === letter) held = { letter, code: e.code };
    invoke('leader_press', { letter }).catch((err) => {
        held = null;
        mark('');
        setStatus(errorMessage(err), 'error');
    });
});

document.addEventListener('keyup', (e) => {
    if (!held || (e.code !== held.code && letterOf(e) !== held.letter)) return;
    e.preventDefault();
    const { letter } = held;
    held = null;
    void invoke('leader_release', { letter });
});

onShown(() => void refresh());
void refresh();
