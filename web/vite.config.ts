/// <reference types="vitest/config" />
import { defineConfig, loadEnv } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, '.', '')
  const apiProxyTarget = env.VITE_DEV_API_PROXY_TARGET ?? 'http://127.0.0.1:3000'

  return {
    plugins: [react()],
    build: {
      chunkSizeWarningLimit: 150,
      rollupOptions: {
        output: {
          manualChunks: {
            'vendor-router': ['react-router-dom'],
            'vendor-virtual': ['@tanstack/react-virtual'],
          },
        },
      },
    },
    server: {
      host: '0.0.0.0',
      port: 4173,
      proxy: {
        '/api': {
          target: apiProxyTarget,
          changeOrigin: true,
        },
      },
    },
    preview: {
      host: '0.0.0.0',
      port: 4173,
    },
    test: {
      environment: 'jsdom',
      setupFiles: './src/test-setup.ts',
    },
  }
})
