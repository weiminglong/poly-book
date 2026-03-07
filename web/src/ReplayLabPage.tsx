import { useCallback, useEffect, useState } from 'react'
import { isAbortError } from './client'
import { getDemoReplayDefaults } from './client'
import { formatNumber, formatTimestamp } from './formatters'
import type { DataSourceMode, ReplayFormValues, ReplayReconstructionResponse, WorkstationClient } from './types'
import { ErrorBanner, MetricCard, OrderBookTable, SectionCard, WarningPanel } from './ui'

interface LoadState<T> {
  data: T | null
  loading: boolean
  error: string | null
}

export default function ReplayLabPage({
  client,
  sourceMode,
}: {
  client: WorkstationClient
  sourceMode: DataSourceMode
}) {
  const [form, setForm] = useState<ReplayFormValues>(() =>
    sourceMode === 'demo'
      ? getDemoReplayDefaults()
      : {
          assetId: '',
          atUs: '',
          mode: 'recv_time',
          depth: 5,
        },
  )
  const [resultState, setResultState] = useState<LoadState<ReplayReconstructionResponse>>({
    data: null,
    loading: false,
    error: null,
  })
  const [assetOptions, setAssetOptions] = useState<string[]>([])

  useEffect(() => {
    const controller = new AbortController()

    const loadAssetOptions = async () => {
      try {
        const assets = await client.getActiveAssets({ signal: controller.signal })
        setAssetOptions(assets.map((asset) => asset.asset_id))
      } catch (error) {
        if (!isAbortError(error)) {
          setAssetOptions([])
        }
      }
    }

    void loadAssetOptions()

    return () => {
      controller.abort()
    }
  }, [client])

  const runReplay = useCallback(
    async (values: ReplayFormValues) => {
      const atUs = Number(values.atUs)
      if (!values.assetId.trim()) {
        setResultState({ data: null, loading: false, error: 'Asset ID is required.' })
        return
      }
      if (!Number.isFinite(atUs) || atUs <= 0) {
        setResultState({
          data: null,
          loading: false,
          error: 'Timestamp must be a positive microsecond value.',
        })
        return
      }
      if (values.depth <= 0) {
        setResultState({ data: null, loading: false, error: 'Depth must be greater than zero.' })
        return
      }

      setResultState((previous) => ({ ...previous, loading: true, error: null }))

      try {
        const replay = await client.getReplayReconstruction({
          assetId: values.assetId.trim(),
          atUs,
          mode: values.mode,
          depth: values.depth,
        })
        setResultState({ data: replay, loading: false, error: null })
      } catch (error) {
        if (isAbortError(error)) {
          return
        }

        const message = error instanceof Error ? error.message : 'Replay reconstruction failed.'
        setResultState({ data: null, loading: false, error: message })
      }
    },
    [client],
  )

  return (
    <div className="page-stack">
      <section className="hero-card">
        <div>
          <p className="section-title">Replay Lab</p>
          <h2>Inspect Parquet-backed reconstruction with explicit time semantics.</h2>
        </div>
        <span className="pill pill--neutral">Parquet only</span>
      </section>

      {resultState.error ? (
        <ErrorBanner
          hint={
            sourceMode === 'api'
              ? 'Replay stays Parquet-backed today, so local history still needs to exist under `./data`.'
              : undefined
          }
          message={resultState.error}
          title="Replay request failed"
        />
      ) : null}

      <div className="two-column-grid">
        <SectionCard
          title="Replay query"
          toolbar={<span className="muted">Foreground-first render path; no background replay polling</span>}
        >
          <form
            className="form-grid"
            onSubmit={(event) => {
              event.preventDefault()
              void runReplay(form)
            }}
          >
            <label>
              <span>Asset ID</span>
              <input
                onChange={(event) => setForm((previous) => ({ ...previous, assetId: event.target.value }))}
                placeholder="tok1"
                type="text"
                value={form.assetId}
              />
            </label>

            <label>
              <span>`at_us`</span>
              <input
                onChange={(event) => setForm((previous) => ({ ...previous, atUs: event.target.value }))}
                placeholder="1700000000000000"
                type="text"
                value={form.atUs}
              />
            </label>

            <label>
              <span>Mode</span>
              <select
                onChange={(event) =>
                  setForm((previous) => ({
                    ...previous,
                    mode: event.target.value as ReplayFormValues['mode'],
                  }))
                }
                value={form.mode}
              >
                <option value="recv_time">recv_time</option>
                <option value="exchange_time">exchange_time</option>
              </select>
            </label>

            <label>
              <span>Depth</span>
              <input
                max={200}
                min={1}
                onChange={(event) =>
                  setForm((previous) => ({ ...previous, depth: Number(event.target.value) || 1 }))
                }
                type="number"
                value={form.depth}
              />
            </label>

            <button className="primary-button" type="submit">
              {resultState.loading ? 'Running…' : 'Run reconstruction'}
            </button>
          </form>

          <div className="chip-list">
            {assetOptions.map((assetId) => (
              <button
                className="chip"
                key={assetId}
                onClick={() => setForm((previous) => ({ ...previous, assetId }))}
                type="button"
              >
                {assetId}
              </button>
            ))}
          </div>

          {sourceMode === 'demo' ? (
            <button
              className="secondary-button"
              onClick={() => setForm(getDemoReplayDefaults())}
              type="button"
            >
              Load demo query
            </button>
          ) : null}

          <p className="muted">
            API mode requires local Parquet history under `./data`. Demo mode provides a seeded
            query plus fixture-backed replay responses.
          </p>
        </SectionCard>

        <SectionCard title="Replay result">
          {resultState.loading && !resultState.data ? <p className="muted">Loading replay reconstruction…</p> : null}
          {!resultState.loading && !resultState.error && !resultState.data ? (
            <p className="muted">Submit a replay query to inspect the reconstructed top of book.</p>
          ) : null}
          {resultState.data ? (
            <div className="snapshot-stack">
              <div className="stats-grid compact">
                <MetricCard label="Asset" value={resultState.data.asset_id} />
                <MetricCard label="Mode" value={resultState.data.mode} />
                <MetricCard
                  label="Checkpoint"
                  value={resultState.data.used_checkpoint ? 'Used checkpoint' : 'Snapshot only'}
                />
                <MetricCard label="Sequence" value={String(resultState.data.sequence)} />
                <MetricCard label="Updated" value={formatTimestamp(resultState.data.last_update_us)} />
                <MetricCard label="Mid" value={formatNumber(resultState.data.mid_price)} />
              </div>

              <OrderBookTable asks={resultState.data.asks} bids={resultState.data.bids} />

              <SectionCard title="Continuity events" dense>
                {resultState.data.continuity_events.length === 0 ? (
                  <p className="muted">No continuity events were returned for this reconstruction.</p>
                ) : (
                  <div className="warning-stack">
                    {resultState.data.continuity_events.map((warning) => (
                      <WarningPanel key={`${warning.kind}-${warning.recv_timestamp_us}`} warning={warning} />
                    ))}
                  </div>
                )}
              </SectionCard>
            </div>
          ) : null}
        </SectionCard>
      </div>
    </div>
  )
}
