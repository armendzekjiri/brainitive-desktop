import { defineConfig } from 'vite';

// The bundled pages only: the connect / sign-in screens (index.html) and the native windows
// (quick capture, quick switcher, settings). The web app itself is loaded from the server.
// Port 1420 is `build.devUrl` in src-tauri/tauri.conf.json.
export default defineConfig({
    clearScreen: false,
    server: {
        port: 1420,
        strictPort: true,
        host: '127.0.0.1',
        watch: { ignored: ['**/src-tauri/**'] },
    },
    build: {
        target: 'safari15',
        outDir: 'dist',
        emptyOutDir: true,
        // no inline scripts / styles: the CSP is script-src 'self'
        assetsInlineLimit: 0,
        modulePreload: { polyfill: false },
        rollupOptions: {
            input: {
                index: 'index.html',
                settings: 'settings.html',
            },
        },
    },
});
