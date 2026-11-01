// The Settings window (menu → Settings…, ⌘, / Ctrl+,). Desktop preferences only; the account's
// own settings stay in the web app ("Account & notifications…").

import { invoke } from '@tauri-apps/api/core';
import { $, onShown } from './native';

interface Prefs {
    app_version: string;
    platform: 'macos' | 'windows' | 'linux';
    server: string | null;
    signed_in: boolean;
    zoom_percent: number;
}

function render(p: Prefs): void {
    $('version').textContent = p.app_version;
    document.querySelectorAll('.mod').forEach((el) => (el.textContent = p.platform === 'macos' ? '⌘' : 'Ctrl+'));
    $('zoom').textContent = `${p.zoom_percent} %`;
    $('server').textContent = p.server ?? 'Not connected';
    $('server-state').textContent = p.server ? (p.signed_in ? 'Signed in' : 'Not signed in') : '';
    ($('open-account') as HTMLButtonElement).disabled = !p.signed_in;
}

async function refresh(): Promise<void> {
    render(await invoke<Prefs>('settings_get'));
}

$('zoom-reset').addEventListener('click', () => void invoke<Prefs>('settings_reset_zoom').then(render));
$('open-account').addEventListener('click', () => void invoke('settings_open_account'));
document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') void invoke('settings_close');
});

onShown(() => void refresh());
void refresh();
