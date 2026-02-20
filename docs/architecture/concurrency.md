# Concurrency Model

Hallucinator's validation engine is designed around a **per-DB drainer pool** architecture that maximizes throughput while respecting per-database rate limits. This document explains the concurrency primitives, task structure, and how they interact.

## Design Goals

1. **Maximize parallelism** — Check multiple references simultaneously
2. **Respect rate limits** — Each database has its own rate limit; never exceed it
3. **Minimize latency** — Return results as soon as a verified match is found
4. **Avoid contention** — No shared rate limiter governor across tasks

## Architecture Diagram

```
                         ┌──────────────────┐
                         │   Job Queue      │
                         │ (async_channel)  │
                         └────────┬─────────┘
                                  │
             ┌────────────────────┼────────────────────┐
             │                    │                    │
      ┌──────▼──────┐     ┌──────▼──────┐     ┌──────▼──────┐
      │ Coordinator │     │ Coordinator │     │ Coordinator │
      │   Task 1    │     │   Task 2    │     │   Task N    │
      └──────┬──────┘     └──────┬──────┘     └──────┬──────┘
             │                    │                    │
             │  Local DBs inline  │                    │
             │  Cache pre-check   │                    │
             │                    │                    │
             └──────────┬─────────┘                    │
                        │ Fan out (cache misses only)  │
         ┌──────────────┼──────────────────────────┐   │
         │              │              │           │   │
   ┌─────▼─────┐ ┌──────▼─────┐ ┌─────▼────┐ ┌───▼───▼──┐
   │ CrossRef  │ │   arXiv    │ │   DBLP   │ │   ...    │
   │ Drainer   │ │  Drainer   │ │ Drainer  │ │ Drainers │
   │           │ │            │ │ (online) │ │          │
   │ Rate: 1/s │ │ Rate: 3/s  │ │ Rate:1/s │ │          │
   └───────────┘ └────────────┘ └──────────┘ └──────────┘
```

## Task Types

### Coordinator Tasks

- **Count:** Configurable via `num_workers` (default: 4)
- **Role:** Pick references from the shared job queue, run local DBs inline, pre-check cache, fan out to drainers
- **Concurrency:** Multiple coordinators run in parallel, each pulling from the same `async_channel`

A coordinator's lifecycle for each reference:

1. Receive `RefJob` from job queue
2. Emit `ProgressEvent::Checking`
3. Query local databases inline (DBLP offline, ACL offline) — sub-millisecond
4. If verified locally → emit result, skip remote phase
5. Pre-check cache for all remote DBs (synchronous, prevents race condition)
6. If verified from cache → emit result, skip drainers
7. Create `RefCollector` (shared aggregation hub)
8. Send `DrainerJob` to each cache-miss DB's drainer queue

### Drainer Tasks

- **Count:** One per enabled remote database
- **Role:** Process DB queries sequentially at the database's natural rate
- **Rate limiting:** Each drainer is the sole consumer of its DB's `AdaptiveDbLimiter`

A drainer's lifecycle for each job:

1. Check early-exit conditions (cancelled, already verified, no DOI for DOI-requiring backend)
2. Acquire rate limiter token
3. Check cache (within the rate-limited query path)
4. Execute HTTP query with timeout
5. Validate authors if title found
6. Update `RefCollector` state
7. Decrement `remaining` counter; if last, finalize the result

### RefCollector

A per-reference aggregation hub, shared (via `Arc`) by all drainers working on that reference:

```
RefCollector
├── remaining: AtomicUsize    # Drainers left to report
├── verified: AtomicBool      # Early-exit flag
├── state: Mutex<AggState>    # Aggregation (held briefly)
│   ├── verified_info
│   ├── first_mismatch
│   ├── failed_dbs
│   ├── db_results
│   └── retraction
└── result_tx: Mutex<Option<oneshot::Sender>>
```

The last drainer to decrement `remaining` to zero calls `finalize_collector()`, which builds the final `ValidationResult` and sends it on the oneshot channel.

## Concurrency Primitives

| Primitive | Purpose |
|-----------|---------|
| `async_channel::unbounded` | Job queue (coordinators) and per-DB drainer queues |
| `AtomicUsize` + `Ordering::AcqRel` | `remaining` counter for lock-free drainer coordination |
| `AtomicBool` + `Ordering::Release/Acquire` | `verified` flag for early exit |
| `Mutex<AggState>` | Per-reference aggregation state (single mutex, held briefly) |
| `tokio::sync::oneshot` | Return channel for each reference's result |
| `CancellationToken` | Graceful shutdown (Ctrl+C handler) |
| `ArcSwap` | Atomic governor swapping during adaptive rate limit backoff |
| `DashMap` | Lock-free concurrent L1 cache reads |

## Cache Pre-Check: Preventing Race Conditions

A subtle race condition exists without the cache pre-check:

1. Reference R is dispatched to CrossRef (drainer A) and arXiv (drainer B)
2. Drainer A finishes first: CrossRef has a match → sets `verified = true`
3. Drainer B sees `verified = true` → skips arXiv query entirely
4. arXiv's result is never cached for reference R

This means future runs will always miss the arXiv cache for this title.

**Solution:** Before dispatching to any drainer, the coordinator synchronously checks the cache for all remote DBs. Cache hits are recorded in `AggState.db_results`, and only cache-miss DBs are dispatched to drainers. This ensures every DB's cached result is always captured regardless of verification order.

## Early Exit

When a drainer verifies a reference:

1. Sets `collector.verified` to `true` (atomic store with Release ordering)
2. Other drainers check this flag before querying (Acquire ordering)
3. Drainers that see `verified = true` emit a `Skipped` status and decrement `remaining`

This avoids unnecessary API calls once a match is found.

## SearxNG Fallback

If a reference is `NotFound` after all remote DBs have been checked and SearxNG is configured:

1. `finalize_collector()` runs a SearxNG web search as a last-resort fallback
2. If SearxNG finds the title, the status upgrades from `NotFound` to `Verified` (source: "Web Search")
3. SearxNG results don't undergo author validation (web search doesn't return structured author data)

## Shutdown Sequence

1. User presses Ctrl+C → `CancellationToken` is cancelled
2. `job_tx` (the job queue sender) is closed
3. Coordinators drain remaining jobs, checking `cancel.is_cancelled()` at each iteration
4. Drainers skip remaining jobs when cancelled
5. When all coordinators finish, they drop their `Arc<drainer_txs>` clones
6. Drainer channels close → drainers drain and exit
7. Pool handle completes

## Performance Characteristics

- **Local DB queries:** < 1ms (SQLite FTS5 lookups)
- **Cache hits:** Sub-microsecond (DashMap L1) to ~1ms (SQLite L2)
- **Remote DB queries:** 100ms–10s depending on database and network
- **Throughput:** Scales linearly with `num_workers` for CPU-bound coordination; drainer throughput is rate-limit-bound per DB
- **Memory:** One `RefCollector` per in-flight reference (small: a few KB each)
