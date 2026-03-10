import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { ReactQueryDevtools } from '@tanstack/react-query-devtools'
import type { ReactNode } from 'react'
import { TooltipProvider } from '../shared/components/tooltip'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 5_000,
      gcTime: 5 * 60_000,
      retry: 2,
      refetchOnWindowFocus: true,
    },
  },
})

export function Providers({ children }: { children: ReactNode }) {
  return (
    <QueryClientProvider client={queryClient}>
      <TooltipProvider>{children}</TooltipProvider>
      <ReactQueryDevtools initialIsOpen={false} buttonPosition="bottom-left" />
    </QueryClientProvider>
  )
}
