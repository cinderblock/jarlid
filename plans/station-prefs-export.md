# Station Preferences Export (and later Import) — Living Plan

Plan path: `plans/station-prefs-export.md`
Related: `plans/pandora-native-client.md` (the protocol client this is built on),
`plans/pandora-desktop-app.md` (the app's history, incl. the webview era)

## Goal

Export the user's Pandora **station preferences** — thumbs up/down, seed songs and
artists, and per-station settings — into a file they own. Import is an explicit
follow-up; the export format is designed for it from day one.

Why: this is the data years of listening produced. It exists only inside the Pandora
account, has no official export, and disappears with the account.

## Decisions already made (don't re-ask)

- **UI:** checkbox multi-select in the **existing station picker** (search filter,
  Select all / none, Export button). A dedicated **Library window** is a later round.
- **Format:** ONE versioned JSON file, `"jarlidExport": 1`. Not CSV, not per-station files.
- **Scope:** thumbs + seeds + per-station settings. Explicitly OUT: dumping raw API
  responses wholesale, and exporting Jarlid's local recently-played history.
- **Data source:** the native tuner client (`crates/pandora`), one
  `station.getStation` call per station. NOT the REST API, and NOT the old
  webview/`bridge.js` path (which no longer exists).

## History worth knowing

This feature was first built against the **engine-webview architecture** (scraping
`bridge.js` + Pandora's `/api/v1` REST endpoints), on a checkout that was 16 commits
stale. Upstream had already replaced the webview with the native client at v1.0.0, so
that work targeted deleted code. It is preserved on the branch
`export-station-prefs-webview` and is **not** the live implementation.

Lesson recorded: `git fetch` and check divergence *before* building, not after.

The rewrite is strictly better anyway — see the request-count note below.

## Architecture

    UI (main.ts)  --invoke export_stations([[name, token], …])-->  export.rs
                                                                     |  serially, per station:
                                                                     |    engine.station_details(token)
                                                                     |    map_station(...)
                                                                     |    emit export://progress
                                                                     v
                                                             save dialog + write file

- One tuner call per station. The REST route needs ~6 (getStationDetails, getSeeds,
  annotateObjectsSimple to resolve seed names, getStationFeedback twice paginated, plus
  modes), so this is roughly a 6× reduction in requests against the account.
- Cancellation is an `AtomicBool` in Tauri state, checked between stations.
- Save dialog + file write in Rust via `tauri-plugin-dialog` (Rust-only dependency: no
  JS package, no capability permission, since the UI never calls the dialog directly).

Contract:
- `invoke("export_stations", { stations: [[name, token], …] })` →
  `{ path, stations, thumbs, seeds, skipped, stoppedReason }`; `path: null` means the
  save dialog was dismissed.
- `invoke("cancel_export")`
- `export://progress` → `{ done, total, station }`

## Pacing / account-safety

An export runs *while music is playing* and Pandora permits one stream per account,
so it must never look like a scrape: strictly serial, 700 ms between stations, and a
`STREAM_VIOLATION` stops the run rather than retrying into it. Never automatic, never
on a timer — user-initiated only.

An early stop (cancel, stream violation, per-station failure) **keeps everything
collected so far** and still offers to save it, reporting `stoppedReason`. Only a run
that collected nothing at all is an error. Discarding 80 stations of deliberately slow
work because station 81 failed would be indefensible.

## The API, verified against the live account 2026-08-08

`station.getStation` with `includeExtendedAttributes: true`. Confirmed with
`cargo run -p engine --example dump-station-shape`, which prints field names and types
only — never anyone's listening data — and is the contract check to re-run if the
export ever starts producing empty fields.

| what | where | confirmed |
|---|---|---|
| header | `stationId`, `stationToken`, `stationName`, `artUrl`, `dateCreated{time}` | yes |
| settings | `isShared`, `isQuickMix`, `isGenreStation`, `allowAddMusic`, `allowRename`, `allowDelete`, `allowEditDescription`, `genre[]`, `hasTakeoverModes`, `hasCuratedModes`, `modes{}`, `quickMixStationIds[]` | yes |
| song seed | `music.songs[]`: `songName`, `artistName`, `musicToken`, `pandoraId`, `seedId`, `artUrl` | yes |
| artist seed | `music.artists[]`: `artistName`, `musicToken`, `pandoraId`, `seedId`, `artUrl` | yes |
| genre seed | `music.genres[]`: name field **UNCONFIRMED** | no — no station on the account had one |
| thumbs | `feedback.thumbsUp[]`: `songName`, `artistName`, `songIdentity`, `pandoraId`, `musicToken`, `feedbackId`, `dateCreated`, `albumArtUrl`, `isPositive` | yes (up) |
| thumbs down | `feedback.thumbsDown[]` | not yet seen non-empty; assumed same record |
| totals | `feedback.totalThumbsUp` / `totalThumbsDown` | yes |

### Things that turned out NOT to exist

- **A thumb carries no `albumName`.** An earlier draft mapped `album` and it would have
  been permanently empty. There is deliberately no `album` field in the schema.
- **`isThumbprintStation` and `requiresCleanAds`** — old partner-API names, absent here.
- **Per-station "allow explicit content"** — account-level only
  (`listener/updateAccount`, `enableExplicitContentFilter`).
- **An artist variety / play-frequency dial** — doesn't exist. Pandora's "Add Variety"
  literally adds a seed. The real per-station dial is the Discovery Tuner (`modes`).
- **QuickMix returns neither `music` nor `feedback`** — it is a shuffle over other
  stations, so `quickMixStationIds` *is* its content and must be exported.

### ⚠️ The import constraint (matters later)

`station.addFeedback` takes a **`trackToken`**, which is ephemeral — issued per playlist
fragment, only for a track Pandora has just served on that station. There is no bulk
"restore my thumbs" call.

So: **export is lossless, import cannot be.**
- seeds — restorable (`station.addSeed` takes `{stationToken, musicId}`);
- stations — recreatable (`station.createStation` from a musicToken), renameable;
- thumbs — **not** directly restorable. Best available approach is to hold the exported
  list and re-apply a thumb opportunistically when that track happens to play.

This is why the export keeps `musicToken` on every thumb: a thumbed song can at least be
turned back into a *seed*, which is the closest thing to restoring it.

## Export file schema (v1)

```json
{
  "jarlidExport": 1,
  "exportedAt": "2026-08-08T…Z",
  "exportedBy": "Jarlid 1.0.0",
  "stations": [{
    "stationId": "…", "stationToken": "…", "name": "…",
    "art": "…", "dateCreated": "1600000000000",
    "counts": { "up": 42, "down": 7 },
    "settings": { "isShared": false, "modes": {…}, "quickMixStationIds": [...], … },
    "seeds":    [{ "kind": "artist", "name": "…", "musicToken": "R…",
                   "pandoraId": "AR:…", "seedId": "…", "art": "…" }],
    "feedback": [{ "rating": "up", "name": "…", "artist": "…", "songIdentity": "…",
                   "pandoraId": "TR:…", "musicToken": "S…", "feedbackId": "…",
                   "dated": "1700000000000", "art": "…" }],
    "warnings": ["Pandora reported 500/0 thumbs up/down but returned 100/0"]
  }]
}
```

- `seeds` and `feedback` are flat arrays with a discriminator (`kind`, `rating`) rather
  than grouped objects: import loops once, and a new seed kind doesn't change the schema.
- `settings` is free-form. The option set is Pandora's to change, and an export that
  silently drops an unmodelled field is worse than one carrying an unfamiliar field.
- `musicToken` / `pandoraId` / `seedId` / `feedbackId` are preserved verbatim — those are
  the only values an import can act on.
- `warnings` records when Pandora's reported totals exceed the rows it actually handed
  over. A backup that looks complete but isn't is the worst possible outcome here.

## Bug found and fixed along the way

**The station picker was silently broken on `master`.** The UI sent
`player_cmd("station:<index>")`, `native_cmd` rejected it as an unknown command, and
`main.ts` swallowed the error with `.catch(() => {})` — so clicking a station did
nothing at all. `native_play_station`/`native_stations` existed but nothing called them,
and `engine://stations` emitted names with no tokens. Fixed here because export
selection needs the tokens anyway: the event now carries `tokens[]` and a row click
invokes `native_play_station`.

## Progress log

- [x] 2026-08-08 Scoped with user; UI / format / scope decided.
- [x] 2026-08-08 First implementation (webview/REST) — **obsolete**, see History.
- [x] 2026-08-08 Branch renamed `main` → `master` on GitHub at the user's request.
- [x] 2026-08-08 Rebuilt on the native client: `pandora::Client::station_details`,
      `Engine::station_details`, `app/src-tauri/src/export.rs`.
- [x] 2026-08-08 Station-picker token plumbing fixed (bug above).
- [x] 2026-08-08 API contract verified live via `dump-station-shape`; mapping corrected
      (dropped `album`, added `musicToken`/`pandoraId`/`dated`, fixed the settings list).
- [x] 2026-08-08 10 unit tests, fixture mirrors the real observed shape. All green,
      `tsc --noEmit` clean.
- [ ] **Not yet exercised end-to-end in the running app** — see below.

## Still to verify

The API layer is confirmed against the live account. What has *not* been run:

1. The full flow in the app: select stations → Export → save dialog → file on disk.
2. Cancel mid-run.
3. The station-picker fix (clicking a station actually switches playback now).
4. Genre-seed name field, and thumbs-down record shape — no station sampled had either.
   The exporter hedges on the genre name and will simply leave it blank if wrong.

## Things not to do

- Don't fan out parallel requests to make export fast. Serial is the point.
- Don't add an automatic/scheduled export.
- Don't run `cargo fmt --all` in `crates/` — that workspace is not rustfmt-clean and it
  reformats ~20 unrelated files. Format only what you touched.
- Don't run `bunx prettier` — no prettier config here; it reformats the whole repo.
