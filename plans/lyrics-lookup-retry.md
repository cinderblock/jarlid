# Lyrics lookup: tell a timeout apart from a miss, and retry

## Goal

A lyrics lookup that fails because LRCLIB was slow or unreachable should not be
reported as "No lyrics found", and it should be tried again while the song is still
playing. Raised 2026-09-19: "Are we handling timeouts on the lyrics lookup the same as
a lyrics not found? we should probably retry."

## Environment / context

- Backend: `app/src-tauri/src/lyrics.rs`, `fetch_lyrics` command. Order is local
  override → disk cache → LRCLIB `/api/get` (exact, needs duration) → `/api/search`
  with the full title, then a simplified title.
- Frontend: `app/src/main.ts`, `loadLyricsFor` → `applyLyrics`. `Lyrics unavailable`
  is the `catch` branch; `No lyrics found` is the `source: "none"` branch.
- `tokio` is already a dependency with the `time` feature (used by `export.rs` and
  `native.rs`), so backoff sleeps cost nothing new.
- Misses are deliberately not cached, so a fresh play of the same track always looks
  again. That is the only "retry" the old code had.

## Findings

- **There was no timeout at all.** The reqwest client was built with only a user agent;
  reqwest has no default request timeout, so a stalled connection left the pane on
  "Loading lyrics…" indefinitely.
- **Transport errors were swallowed into "not found".** Every request sat in
  `if let Ok(resp) = …send().await` and `if resp.status().is_success()`, so a
  connection error, a 5xx, or a 429 fell through to the next step and finally to
  `source: "none"`. The UI rendered that as "No lyrics found".
- The frontend's `Lyrics unavailable` branch was effectively dead: the only way for the
  command to return `Err` was `reqwest::Client::builder().build()` failing.
- LRCLIB signals a genuine miss differently per endpoint: `/api/get` returns 404 with
  `{code, name, message}`; `/api/search` returns 200 with `[]`.

## Decisions already made (don't re-ask)

- Retry lives in the backend, per request: a request that fails for a transient reason
  (transport error, timeout, 429, 5xx) is retried with a short backoff before the
  command gives up. Timeout per attempt: 10 s (LRCLIB is slow but not that slow).
- Once one request has exhausted its retries the command returns `Err` immediately
  rather than moving on to the next lookup step. The next step would hit the same
  outage, and three steps × three attempts × 10 s is too long to hold the pane.
- A 404 from `/api/get` is "no such record", not an error, and moves on to search.
  Other 4xx are surfaced as `Err` without retry: retrying a bad request changes nothing.
- The frontend, on `Err`, shows "Lyrics unavailable" and schedules a bounded number of
  later attempts (20 s, then 60 s) while the same track is still playing. Not an
  unbounded loop: LRCLIB is a volunteer-run public service.

## Plan / steps

1. [x] Read the current flow and write this plan.
2. [x] `lyrics.rs`: client timeout, a `lrclib_get` helper that classifies replies and
   retries transient ones, `fetch_lyrics` rewritten on top of it.
3. [x] `main.ts`: log the error, guard the `catch` on the current key, schedule the
   deferred retries, cancel them on track change.
4. [x] `cargo test` in `app/src-tauri`, `bun run build` in `app/`.
5. [x] README: mention that lookups retry rather than reporting a miss.
6. [x] Commit.

## Progress log

- 2026-09-19: all steps done; see the commit that adds this file.

## Things not to do

- Don't cache a failed lookup, and don't cache a miss either: the whole point is that
  the next attempt is a real one.
- Don't retry across the whole `fetch_lyrics` command from the frontend on a tight
  loop. Per-request retries in the backend plus the two deferred retries are the budget.
