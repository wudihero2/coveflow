import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig, loadEnv } from 'vite';

export default defineConfig(({ mode }) => {
	const env = loadEnv(mode, '.', 'COVEFLOW_');
	const apiProxyTarget = env.COVEFLOW_API_PROXY_TARGET ?? 'http://127.0.0.1:8000';

	return {
		plugins: [tailwindcss(), sveltekit()],
		build: {
			// Monaco is lazy-loaded by ScriptEditor and intentionally larger than app chunks.
			chunkSizeWarningLimit: 2500
		},
		server: {
			host: '127.0.0.1',
			port: 5173,
			strictPort: true,
			allowedHosts: ['localhost', '127.0.0.1'],
			proxy: {
				'/api': {
					target: apiProxyTarget,
					changeOrigin: true
				},
				'/health': {
					target: apiProxyTarget,
					changeOrigin: true
				}
			}
		}
	};
});
