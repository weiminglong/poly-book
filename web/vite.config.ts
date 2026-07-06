/// <reference types="vitest/config" />
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig, loadEnv } from 'vite'

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, '.', '')
  const apiProxyTarget = env.VITE_DEV_API_PROXY_TARGET ?? 'http://127.0.0.1:3000'
  const devHost = env.VITE_DEV_HOST ?? '127.0.0.1'

  return {
    // Non-root base for GitHub Pages project-site builds
    // (https://<owner>.github.io/poly-book/); defaults to root everywhere else.
    base: env.VITE_BASE_PATH ?? '/',
    plugins: [react(), tailwindcss()],
    build: {
      chunkSizeWarningLimit: 150,
      rollupOptions: {
        output: {
          manualChunks: {
            'vendor-router': ['react-router-dom'],
            'vendor-virtual': ['@tanstack/react-virtual'],
            'vendor-query': ['@tanstack/react-query'],
          },
        },
      },
    },
    server: {
      host: devHost,
      port: 4173,
      proxy: {
        '/api': {
          target: apiProxyTarget,
          changeOrigin: true,
          // Proxy WebSocket upgrades too, otherwise the orderbook stream
          // (/api/v1/streams/orderbook) is dead in dev and e2e.
          ws: true,
        },
      },
    },
    preview: {
      host: devHost,
      port: 4173,
    },
    test: {
      environment: 'jsdom',
      setupFiles: './src/test-setup.ts',
      exclude: ['e2e/**', 'node_modules/**'],
      coverage: {
        provider: 'v8',
        reporter: ['text', 'lcov'],
        reportsDirectory: './coverage',
        exclude: [
          'e2e/**',
          '**/__tests__/**',
          '**/*.test.{ts,tsx}',
          '**/*.d.ts',
          'src/test-setup.ts',
          'vite.config.ts',
          'playwright.config.ts',
        ],
      },
    },
  }
})
