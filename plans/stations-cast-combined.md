# One button for stations, and cast on every row — Living Plan

Plan path: `plans/stations-cast-combined.md`

Status: **done.** Feature in `bd3f021`; v1.9.3 bumped and tagged after it.

Follows `plans/stations-modal.md`, which put the station name top-right beside the
all-stations button.

## Goal

Three asks from the user on 2026-09-19:

1. The station name pill and the "all stations" button both open the Stations panel. Combine
   them into one control.
2. The "cast" (speakers) button folds in too: every station row on the Stations page gets its
   own cast button, and the separate preset popover goes.
3. Cut a new patch version (v1.9.3) when done — its own version-only commit, then the tag.

## Environment / context

- `app/` — Vite + TS. `bun run build` = `tsc && vite build`; the build is the gate. Rust side
  at `app/src-tauri/` (`cargo check` there, not the repo root).
- Casting is WiiM/LinkPlay presets: Jarlid cannot start an arbitrary Pandora station on the
  speaker. `remote_presets` lists the device's presets (name, source, art, number);
  `remote_cmd` `preset:N` fires one. Generic UPnP renderers have no presets.
- The release workflow runs on a pushed `v*` tag; the version commit is what the tag points at.

## Decisions already made (don't re-ask)

- **One pill: list icon + station name.** The name is empty before anything plays, and hidden
  in remote mode, so the pill collapses to the round icon button on its own. No fallback
  label — the icon is the label.
- **A row's cast button plays the preset whose name matches the station** (case-insensitive,
  trimmed; presets whose `source` names another service are skipped). Every row still gets
  the button; one with no matching preset is dim and clicking it says how to fix that in the
  WiiM Home app, in the panel's status line, rather than silently doing nothing.
- **Casting pauses local playback.** Asking the speakers to play means moving playback there;
  otherwise two sources run at once and the display stays on the local engine.
- **Cast buttons only appear when a LinkPlay renderer is discovered** and its presets were
  fetched. They hide in select mode, where the page is about picking.
- **Non-Pandora presets are no longer reachable from Jarlid.** The old popover listed every
  preset; the combined design only lists stations. Accepted loss, flagged to the user.
- Feature is one commit; the version bump is a separate version-only commit (`CLAUDE.md`).

## Plan / steps

1. [x] Plan written.
2. [x] `index.html`: merge `#stations-btn` into `#station` (icon + `#station-name` span);
   remove `#speakers-btn` and `#speakers-panel`.
3. [x] `main.ts`: name painting and flight use the span; drop the stations-btn and preset
   panel code; forward the remote device name to the stations page.
4. [x] `stations-page.ts`: rows become `div[role=button]` (a button cannot hold a button);
   fetch presets on device change and on open; per-row cast button; wide layout column.
5. [x] `styles.css`: pill with icon, remote-mode hides the name only, cast button styles,
   drop the popover styles, extra grid column.
6. [x] README: the preset-list sentence.
7. [x] `bun run build`; commit feature; version bump commit; tag; push.

## Findings / gotchas

- `.sp-row` was a `<button>` containing an `<input>` in select mode (already invalid nesting);
  the cast button forces the switch to a `div` with keyboard handling.
- Opening the page re-queries presets, and the station name is mid-flight into its row at
  that moment. Re-rendering on every answer detached the landing row; the page now
  re-renders only when the preset list actually changed.
- Verified in a plain browser via Vite on port 1430: the pill is exactly 34×34 with no
  name and grows to a pill with one. The cast buttons could not be exercised there — they
  need a WiiM answering `remote_presets` — so that path is unverified against hardware.

## Progress log

- [x] Explored the code, wrote this plan.
- [x] Feature implemented, built, committed as `bd3f021`.
- [x] v1.9.3 version-only commit and tag.

## Things not to do

- Don't bump the version inside the feature commit.
- Don't run bare `cargo fmt`.

## Follow-up: is there a better WiiM API than presets? (2026-09-19)

Researched after the release. Public documentation gives two layers:

- **HTTP API (what Jarlid uses).** `getPresetInfo` (name/source/picurl, no station id),
  `MCUKeyShortClick:N` / `setPlayerCmd:playLocalList:N` to fire a preset, `setPlayerCmd:play:<url>`
  for a raw stream. Nothing addresses a Pandora station. Official PDF and forum threads agree;
  staff answer to "start a service playlist from the API" was "use a preset".
- **UPnP `urn:schemas-wiimu-com:service:PlayQueue:1` (port 59152).** Much richer, undocumented
  by WiiM, signatures known from `shumatech/wiimplay/upnp`: `GetKeyMapping`/`SetKeyMapping`
  (presets as XML; a Pandora preset shows `<Source>Pandora2</Source>` and a `Name_#~timestamp`
  name), `BrowseQueue`/`CreateQueue`/`PlayQueueWithIndex` (queue contexts with DIDL-Lite
  tracks), and service-aware `GetUserFavorites(AccountSource, MediaType, Filter)`,
  `GetQueueOnline(QueueName, QueueID, QueueType, ...)`, `SearchQueueOnline`. For Tidal/Qobuz
  people use these to play by service id. No public example of the Pandora station-list or
  station-start call exists; the WiiM Home app must use one, since the device holds the login.

Next step if pursued: probe the real device read-only — `GetKeyMapping`, `BrowseQueue("TotalQueue")`,
`GetUserFavorites("Pandora2", …)` — and capture what the WiiM Home app sends when it starts a
station. Only then can Jarlid start any station rather than only preset ones.

## Probe results (2026-09-19, WiiM Pro Plus "Tom Sawyer Labs - Warehouse", fw Linkplay.4.8.827634)

Device found by SSDP at 10.255.14.34; PlayQueue control URL `/upnp/control/PlayQueue1`,
SCPD `/upnp/PlayQueueSCPD.xml` (35 actions; the full list is in the SCPD, also `SetRating(Source,
TrackID, Rating)` — Pandora thumbs through the speaker — and `GetUserInfo("Pandora2", 0)`,
which returns the device's live Pandora session incl. an access token).

**A Pandora station on the WiiM is a queue context, and its id is Jarlid's tuner token.**
`BrowseQueue("CurrentQueue")` for a Pandora preset gives a `<PlayList>` whose `<ListInfo>` has
`<SourceName>Pandora2</SourceName>`, `<SearchUrl>wiimu_search://<id></SearchUrl>`,
`<ContentType>station</ContentType>`, `<Login_username><pandora userId></Login_username>` and
empty `<Tracks>`. Checked all five Pandora presets against `Client::station_list()`: the
`wiimu_search` id equals `station_token` (and `station_id`) every time.

**Starting any station works.** `CreateQueue(<that XML with a non-preset token>)` then
`PlayQueueWithIndex(name, 1)` started "Control Radio" (not a preset) within 2s; the device
filled `<Tracks>` itself with Pandora audio URLs. So Jarlid can cast every station, not just
preset ones, and the "dim button, go make a preset" state can go. The device fetches audio
from Pandora itself — Jarlid still streams nothing.

Minimal queue context that worked (ListInfo fields copied from a device-made one):
ListName, SourceName=Pandora2, SearchUrl=wiimu_search://TOKEN, Login_username=USERID,
MarkSearch=0, TrackNumber=0, TotalNumber=0, Quality=0, requestQuality=High, UpdateTime=0,
LastPlayIndex=1, UserId=0, StationBackup=1, ContentType=station, SwitchPageMode=0,
CurrentPage=0, TotalPages=0, searching=0, PressType=0, Volume=0; `<Tracks></Tracks>`.
The Pandora userId comes from `GetBasicUserInfo()` → `streamServices[id=Pandora2].userId`
(it is also in every Pandora queue's Login_username). Unknown whether Login_username is even
required; not tested.

**Security finding, told to the user:** `BrowseQueue("TotalQueue")` returns a legacy list
named "Pandora" (SearchUrl `tuner.pandora.com`) containing the Pandora e-mail *and password in
plain text*, readable by anything on the LAN with no auth. Not written down here. Deleting
that queue (`DeleteQueue("Pandora")`) or changing the Pandora password is the user's call.

**Dead ends:** `GetUserFavorites("Pandora2", …)` fails with "UserRegister failed" for
MediaType station/stations/Station/Stations/playlist and "Action Failed" for "". So the
station *list* still comes from Jarlid's own Pandora session, which is fine — it has one.
`GetUserAccountHistory("Pandora2", 10)` → "GetUserInfo failed". Whether a queue's name must be
unique / whether `CreateQueue` on an existing name replaces it: untested (there is
`ReplaceQueue` and `DeleteQueue`).

**Implementation sketch (not started):** in `upnp.rs`, keep the PlayQueue control URL from the
device description; add `cast_station(name, token)` = SOAP `CreateQueue` + `PlayQueueWithIndex`;
expose as `remote_cmd` `station:<token>` or a new command; the Stations page drops preset
matching and `castable()` becomes "device has the PlayQueue service". Presets no longer
needed at all for casting.

## Implemented direct station casting (2026-09-19)

Replaced preset-matching with real per-station casting over the WiiM PlayQueue service.

- `app/src-tauri/src/upnp.rs`: `Target::LinkPlay` gains `pq_ctrl` (the PlayQueue control URL,
  parsed from the device description and resolved against the SSDP location). `RemoteState`
  gains `can_cast` (true when that URL was found). New `play_station(name, token)` builds the
  station queue context, calls `CreateQueue` then `PlayQueueWithIndex` via a wiimu-namespaced
  SOAP helper; `pandora_user_id()` reads the account's Pandora userId from `GetBasicUserInfo`
  to fill `Login_username`, matching a device-made queue.
- `app/src-tauri/src/lib.rs`: `remote_play_station { name, token }` command, registered.
- `app/src/main.ts`: `RemoteState.canCast`; forwarded via `setRemoteDevice(name, canCast)`.
- `app/src/stations-page.ts`: dropped presets, `refreshPresets`, `presetFor`, and the
  unmatched/dim state. `castable()` = device present AND `canCast` AND not select mode. The
  cast button always acts; `cast()` calls `remote_play_station` then pauses local playback.
- `app/src/styles.css`: removed `.sp-cast.unmatched`. README updated.

Verification: frontend `tsc`+build and `cargo check` both pass. The SOAP sequence itself was
verified live against the warehouse WiiM during the probe (CreateQueue + PlayQueueWithIndex
started a non-preset station in ~2s). The compiled app has NOT yet been run end-to-end against
the device — the Rust envelope is byte-equivalent to the verified probe (same actions, same
context XML, same double-escaping), but a live run through the app is still worth doing.

### Note on `cargo fmt`
Bare `rustfmt`/`cargo fmt` wants to wrap the file's established one-liner idiom
(`ctl.target.lock().await.clone().ok_or(...)?`) used throughout upnp.rs. The file is not
default-rustfmt formatted; new code matches the surrounding style deliberately. Don't run bare
`cargo fmt` here.

## UI revision + v1.10.0 (2026-09-19)

The combine had gone too far: the station name and all-stations icon folded into one pill
that showed only an icon when nothing played, and the top-bar cast button was removed
entirely. Per the user, reverted to two clear controls in the top bar:

- `#station`: a text pill showing the current station name (fallback "Stations" so it is
  never blank), opening the Stations panel. The redundant separate list-icon button stays
  merged in.
- `#speakers-btn`: a separate cast button, shown only when `canCast`, opening the same
  Stations panel. Per-row casting on that page is unchanged.

Files: `app/index.html`, `app/src/main.ts`, `app/src/styles.css`. Verified in the browser
via Vite (name pill + cast button + settings, in that order). Committed `ba72f77`.

Then cut **v1.10.0** (minor) as a version-only four-file commit `f0afabe`, tagged and pushed;
release workflow running.

## Remote mode shows the WiiM's station (2026-09-19)

`BrowseQueue("CurrentQueue")` names the station the speaker is on (`ListName`, minus the
`_#~timestamp` a preset-made queue carries) and its token (`SearchUrl`), when `SourceName` is
`Pandora2`. `upnp.rs`: `current_station()`; the poll loop asks on a title change or every 10 s
and copies the answer into `RemoteState.station` / `station_token`. `main.ts`:
`reflectRemoteStation()` puts that name in the station button during remote mode, marks the
row in the Stations page, and restores Jarlid's own station (name + `activeToken`) on exit;
the button hides in remote mode only for a non-Pandora queue. The CSS rule that hid the
button in remote mode is gone. Verified read-only against the device: the queue fields are
as expected (name with suffix, Pandora2, wiimu_search token). Not yet run end-to-end in the
compiled app. Committed with the feature; no version cut.
