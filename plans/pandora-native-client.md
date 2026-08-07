# Pandora Native Client — Reverse-Engineered Reimplementation

Plan path: `plans/pandora-native-client.md`
Sibling plan (the existing web-wrapper app): `plans/pandora-desktop-app.md`

## Goal

Replace Jarlid's **engine webview** (an embedded `pandora.com` browser we scrape and click) with a
**genuine reimplementation of Pandora's client protocol**: speak Pandora's own API directly from
Rust, decode and play the audio ourselves, and own the entire interface. No browser driving the
session, no DOM scraping, no facade over Pandora's web UI.

This **reverses a decision recorded in the sibling plan** (2026-07-08: "We are NOT using the
reverse-engineered partner API — ban risk + fragile"). The user explicitly asked for the
reimplementation on 2026-08-06. The ban-risk tradeoff below is now accepted knowingly, not
overlooked.

## Status

**Protocol crate scaffolded and validated against the live tuner API. Blocked on the user running
the credentialed probe** (step 1 of the plan) — it needs their Pandora password, which is supplied
via environment variables so it never enters a file or a transcript.

## Decisions (locked 2026-08-06/07)

1. **Auth path: tuner login, then test REST token reuse** (option A, falling back to C).
   Blowfish `auth.userLogin` on `tuner.pandora.com` — verified alive, no bot wall. Then test the
   `[STALE 2021]` claim that the resulting `userAuthToken` is accepted as REST `X-AuthToken`.
   If it is: fully browser-free with the rich modern API. If not: fall back to option C, a
   one-time browser login whose token we lift.
2. **Scope: swap the engine, keep Jarlid.** The webview engine is one replaceable module.
   Lyrics sync/cache, SMTC, taskbar thumb toolbar, UPnP/WiiM remote, auto-update and window state
   are all protocol-agnostic and stay.
3. **Audio decode: Windows Media Foundation.** Decodes HE-AAC (incl. SBR/PS) natively with no
   extra shipped dependency; the app is already Windows-only and already uses `windows-rs` for
   SMTC and the thumbbar. Symphonia is rejected — no SBR/PS, would decode dull and half-bandwidth.

**Note for the user, recorded so it is not re-explained:** PerimeterX (now HUMAN Security, after
merging with White Ops in 2022) is a commercial bot-detection service, same category as Cloudflare
Turnstile. It scores requests on browser/TLS fingerprint and behaviour. Our `403 s2s_high_score`
means "server-to-server bot score too high". Pandora runs it **only** in front of `auth/login`,
which is exactly why we authenticate on the tuner API instead.

---

## Protocol research (verified 2026-08-06, live probes + dated sources)

Everything in this section was confirmed by live requests against production Pandora on
2026-08-06 unless tagged otherwise. `[STALE]` = was true years ago, not re-verified.
`[UNCONFIRMED]` = could not be checked (mostly because it needs a paid account).

### Two viable API surfaces

**1. Web REST API** — `www.pandora.com/api/v1/...`. What pandora.com's own player calls.
   - Auth headers: `X-CsrfToken` (matching the `csrftoken` cookie from `GET /`) + `X-AuthToken`.
     **The CSRF values are not actually validated** — the literal string `abc123` worked for both.
   - **135 versioned endpoints** extracted from today's shipping bundle
     (`web-app.b9c804ba28de212a692b.js`). Far richer than the public docs:
     - `v1/auth/{login,anonymousLogin,deviceLogin,logout}`
     - `v1/playlist/{getFragment,narrative}`
     - `v1/station/{getStations,getStationDetails,createStation,addFeedback,deleteFeedback,addSeed,getSeeds,getFeedback,shuffle,trackStarted,playbackPaused,playbackResumed,removeStation}`, `v3/station/addTiredSong`
     - `v1/action/{skip,pause,previous,seek,replay,repeat,shuffle,snooze,thumbUp,thumbDown,removeThumb,mode}`
     - `v1/playback/{current,item,itemList,midroll,peek,source}`
     - `v1/event/{started,progress,ended,offlineEvents}`
     - `v4/catalog/*`, `v1/search/fullSearch`, `v1/listener/*`, `v1/graphql/graphql`
     - `v1/ad/*` (incl. `startValueExchange`, `useSkipReward`, `useReplayReward`)
     - `action/*`, `playback/*`, `event/*`, and all of `ad/*` appear in **no public documentation**.
   - Public doc <https://6xq.net/pandora-apidoc/rest/> is real but its repo last committed
     **2021-04-19** — `[STALE]`, use the bundle as ground truth.

**2. Tuner / partner API** — `tuner.pandora.com/services/json/`, Blowfish-encrypted, hardcoded OEM
   partner credentials. What pianobar / Pithos / pydora use.
   - **Verified alive 2026-08-06**: `auth.partnerLogin` with the published `android` /
     `AC7IBG09A3DTSYM4R41UJWL07VLN8JI7` credentials → `HTTP 200`, valid `partnerAuthToken`.
   - **No Cloudflare, no PerimeterX, no bot wall** on this host. Pandora-owned IPs.
   - pianobar maintainer confirmed it working 2026-03-27 (issue #764); `mopidy-pandora` actively
     ported to Mopidy 4.0 in 2026-04/05 — a maintainer would not do that on a dead backend.

### 🔴 The crux: credentialed login is bot-walled (REST only)

- `POST /api/v1/auth/login` with credentials → **`HTTP 403`**, `Server: envoy`, body
  `{"errorCode":1215,"errorString":"s2s_high_score","appId":"PXXljWHHUe",...}` + `_pxhd` cookie.
  That is **PerimeterX / HUMAN Security**. `s2s_high_score` = server-to-server bot score exceeded.
- **The wall is only on `auth/login`.** With a bogus token, `station/getStations` returns a clean
  `401`, not a PX challenge. No TLS/JA3 fingerprinting observed — stock `curl` with default TLS and
  no browser UA sailed through every non-login endpoint.
- `POST /api/v1/auth/anonymousLogin` with `{}` → **`HTTP 200`** with a real `authToken` and
  **no PX challenge**. Verified to then drive `getStations`, `fullSearch`, `createStation`,
  `getFragment`. **But that is a free anonymous listener — it is not the user's paid account.**
- `[STALE, 2021]` Credentialed login additionally wanted `OZ_TC`/`OZ_DT`/`OZ_SG` params generated by
  White Ops "Hoplon" (RC4-variant; reverse-engineered implementation exists in apidoc issue #45).
  `s.hoplon.pandora.com` still resolves today. White Ops + PerimeterX merged into HUMAN Security in
  2022. `[UNCONFIRMED]` whether `OZ_*` is still required *on top of* PX.
- `[STALE/UNVERIFIED]` A 2021 report claims a `userAuthToken` from the **tuner** API can be used
  directly as `X-AuthToken` on the **REST** API, sidestepping the walled login. One corroboration,
  one failure to reproduce. Architecturally consistent with what we observed. **Worth testing early
  — if true it is the cleanest possible answer**, since it means one Blowfish login unlocks the
  entire modern REST surface with no browser and no PX.

### Audio: no DRM

- `playlist/getFragment` returns `audioURL` =
  `https://t1-5.p-cdn.us/access/?version=5&lid=<listenerId>&token=<~200-char signed token>`.
  Plain HTTPS, signed/expiring query params. Range requests supported (`HTTP 206`).
- Fetched the first 8 KiB directly: `Content-Type: audio/mp4`, `ftypisom`/`moov`, boxes `stsd`
  `mp4a` `esds` **present**; `pssh` `sinf` `schm` `tenc` `enca` `moof` **absent**.
  → **Plain unencrypted progressive MP4/AAC. No CENC, no Widevine, no HLS/DASH, no EME.**
- Encoding `aacplus`, ~64.6 kbit/s **HE-AAC** on the anonymous tier. (Paid tiers advertise up to
  192 kbit/s.)
- Zero EME anywhere in the web client — grepped `web-app`, `web-vendor`, `sxm-audio-player`,
  `media-element-harness`: no `requestMediaKeySystemAccess`, `setMediaKeys`, `com.widevine`,
  `playready`, `fairplay`.
- ⚠️ **An XOR path still ships.** `sxm-audio-player.*.js` contains `XORCipher` / `XOR_MASK`, applied
  **only when the fragment response carries a `key` field**. No `key` appeared on the anonymous
  tier. `[UNCONFIRMED — needs a paid account]` whether Premium/on-demand content sets it. Note this
  is a single-key XOR obfuscation, *not* real DRM — trivially reversible given the `key`.

### No reporting obligation

- Called `playlist/getFragment` **4× consecutively** without ever calling `trackStarted`, any
  `event/*`, or fetching the audio. Every call returned fresh tracks. Playlists keep flowing.
- pianobar has **zero** reporting code and has worked for 15 years.
- The surface exists anyway (telemetry/royalty): per-track `audioReceiptURL` and `audioSkipUrl`,
  plus `station/trackStarted`, `playbackPaused/Resumed`, `event/{started,progress,ended}`.
  We should probably send these anyway — cheap, and makes our traffic look like a real client.
- Session limits (anonymous `config`): `inactivityTimeout: 14400` (4 h), `dailySkipLimit: 48`,
  `stationSkipLimit: 6`. Server-enforced. No keepalive/heartbeat required.

### Why the legacy API survived — structural explanation

The PerimeterX block response's `blockScript` parameter base64-decodes to
`http://localhost:5811/minos/json?method=auth.login`. The REST API is an **Envoy-fronted
translation layer over an internal service ("minos") that still speaks the old tuner-style
JSON-RPC shape**. That is a strong reason to expect the tuner API to keep working.

Separately: Pandora's web player *is* the SiriusXM player now (webpack chunk literally named
`sxm-audio-player`, `SXMHarness`, Hoplon site id `SXMP`). The SXM migration already happened on the
web client and **did not break the legacy API**. No announced Pandora shutdown; still a separate
reported SEC segment (~41.1M MAU / 5.6M subs as of 2025-12-31).

### Ecosystem state

| Project | API | Last activity | Health |
|---|---|---|---|
| pianobar | tuner | tag 2024-12-21, commit 2025-12-05 | ✅ maintainer confirms working 2026-03 |
| mopidy-pandora | tuner (via pydora) | 2026-05-16, Mopidy 4.0 support | ✅ active |
| pydora | tuner | 2024-05-20 | 🟡 quiet, no breakage issues |
| Pithos | tuner | 1.6.2 (2024-03), commit 2026-01 | 🟡 quiet |
| Illusion137/lib-origin | **REST** | 2026-08-06 | 🟡 active but **scraping only** — assumes an existing browser session, no login, no audio |
| mousiki (Go), milkshake (Swift) | REST | 2021 / 2022 | ❌ dead |

**There is no healthy open-source REST-API client with login + audio.** We'd be building the
first one. That cuts both ways: no prior art to copy, but also a genuinely novel result.

### Known breakage reports 2025–2026 (all transient/local, none fatal)

| Date | Report | Outcome |
|---|---|---|
| 2025-07-12 | pianobar #761 **and** pithos #720, same day: SSL cert expired | Pandora-side expired cert, self-resolved in 1–2 days |
| 2025-11 → 2025-12 | pithos #724: intermittent `1001 Invalid username/password` + `Error Code: 9`, 3 machines, clears on restart | **Still open, unresolved.** Speculation: `syncTime` drift or per-IP throttling |
| 2026-03-22 | pianobar #764: ~1-in-100 login success on Android/Termux | Open, platform-specific |

---

## ⚠️ Risk disclosure (accepted by user, recorded here so it is never re-litigated silently)

Pandora's [Terms of Use](https://www.pandora.com/legal) (last updated 2024-04-16) state that
access "through an application, service, or method provided by a party other than Pandora or one of
our licensed third parties, is strictly prohibited" and "may subject your account to termination."

- **No confirmed account bans from tuner-API use were found in 2025–2026** — searched issue
  trackers, community.pandora.com, general web. pianobar users have run it for 15 years.
- So the risk is **contractual, not observably enforced**. But it is a **paid** account.
- ⚠️ A claim circulating that "pianobar complies with ToS §4.2 (automated access permitted for
  personal use)" is **false** — it traces to an AI-generated SEO site. No such clause exists.
  Do not repeat it as justification.

This is personal interop work on the user's own paid account, in the same category as pianobar /
Pithos / pydora. It is not redistribution and does not circumvent DRM (there is none to
circumvent). Keep it that way: no content redistribution, no sharing of audio URLs, no bypassing
tier limits (skips, on-demand).

---

## Open decisions — BLOCKING, ask before building

### 1. Auth path (the only real technical unknown)

| Option | How | Pros | Cons |
|---|---|---|---|
| **A. Tuner login → REST token** | Blowfish `auth.userLogin` on tuner API, then try that `userAuthToken` as REST `X-AuthToken` | Fully native, no browser at all; no PX; unlocks the rich REST surface | The token-reuse trick is `[STALE 2021]`, may not work; impersonates an OEM device (highest ToS exposure) |
| **B. Tuner API only** | pianobar's exact protocol, end to end | Verified working *today*; 15 years of prior art; no bot wall | Old surface — no on-demand, possibly capped bitrate; misses `action/*`, `catalog/*` richness |
| **C. One-time browser login → lift token** | Keep a webview *only* for the login page; extract `authToken`; everything else native REST | Real paid-tier session, traffic looks like the web client (lowest ban risk); PX is satisfied by a genuine browser | Not 100% browser-free — a webview lingers for login / token refresh |
| **D. Reverse the PX + `OZ_*` login signing** | Reimplement the anti-bot signing in Rust | Purest result; zero browser | Active arms race, will break; highest effort by far |

**Recommendation: A first (one afternoon to test the 2021 token-reuse claim — it's the highest-value
unknown), fall back to C.** C is the durable answer if A fails; D is not worth it.

### 2. Scope

Reuse Jarlid's UI/lyrics/SMTC/thumbbar/UPnP work behind a new native engine, or start clean?
**Recommendation: reuse.** The engine webview is one replaceable module — `bridge.js` + the `eval`
plumbing in `lib.rs`. Everything else (lyrics sync + cache, SMTC session, taskbar thumb toolbar,
UPnP/WiiM remote, auto-update, window state) is protocol-agnostic and represents most of the work
to date. Define a `PandoraEngine` trait, implement it natively, keep the webview engine behind a
feature flag until the native one is at parity.

### 3. Audio decode stack ⚠️ non-obvious gotcha

The stream is **HE-AAC** (`aacplus`, AAC-LC + SBR, possibly +PS). **Symphonia — the obvious pure-Rust
choice — does not implement SBR/PS.** Decoding HE-AAC with it yields the AAC-LC core only: half the
sample rate, no high band, audibly dull. Options:

- **Windows Media Foundation** — decodes HE-AAC natively, zero extra deps, already Windows-only app.
  **Recommended.**
- **libmpv / ffmpeg bindings** — bulletproof and portable, but a large native dependency to ship.
- **Symphonia** — only if we can request an AAC-LC/MP3 variant instead (the tuner API's
  `audioUrlMap` does expose an `mp3` encoding; the REST tier's options are `[UNCONFIRMED]`).

---

## Plan / steps (draft — sequence assumes decisions above)

1. **Spike (gates everything):** Rust probe that does tuner `auth.userLogin` with the real paid
   account, then tries that token as REST `X-AuthToken` against `station/getStations`.
   Result determines option A vs C. Document the outcome here either way.
2. Spike: fetch one `audioURL` and decode it through the chosen audio stack; confirm full-bandwidth
   HE-AAC output. Check whether a paid fragment carries a `key` field (XOR).
3. Extract a `pandora` Rust crate: auth, station list, playlist fragments, transport actions,
   feedback, search. Typed structs, no scraping.
4. Native playback: prefetch-next, gapless-ish, position reporting for lyric sync.
5. Define `PandoraEngine` trait; implement `NativeEngine`; keep `WebviewEngine` until parity.
6. Wire existing UI/SMTC/thumbbar/lyrics to the native engine (they consume events, not DOM).
7. Send the telemetry endpoints (`trackStarted`, `event/*`, `audioReceiptURL`) so traffic looks
   like a real client.
8. Delete the engine webview and `bridge.js`. Update README (the "Why this approach" section
   currently argues *for* the wrapper and will be wrong).

## Findings / gotchas (running log — add negative results here!)

- 2026-08-06 Research complete (above). Nothing built yet.
- 2026-08-07 **Live `auth.partnerLogin` succeeded from our own Rust implementation.** This is a
  stronger result than it looks: the response's `syncTime` is Blowfish-encrypted, so decrypting it
  proves our codec, key selection and hex handling are all byte-correct against the real server.
  The tuner API is confirmed alive and our crypto is confirmed right. Run
  `cargo run --bin probe` in `crates/pandora` to re-verify at any time — with no credentials set it
  performs exactly this check and stops.
- 2026-08-07 Deviated from pydora on one detail deliberately: pydora extracts `syncTime` with a
  hardcoded `[4:-2]` slice, which assumes a fixed padding length. We skip the 4 junk bytes and then
  take ASCII digits, which is robust to padding changes. Covered by a unit test.
- Jarlid **already** calls the REST API in one place: `bridge.js refreshStationList()` POSTs
  `/api/v1/station/getStations` with `X-CsrfToken` lifted from the session (sibling plan line 356).
  That is a working, in-production proof that REST + a session token works for this account.
  **Start there when validating option C.**

## Things not to do

- Do not trust <https://6xq.net/pandora-apidoc/> details as current — 2021-vintage. Use the shipping
  JS bundle as ground truth.
- Do not reach for Symphonia for HE-AAC without testing SBR output first (see decision 3).
- Do not repeat the false "pianobar complies with ToS §4.2" claim.
- Do not build option D (PX signing reimplementation) — arms race, guaranteed to rot.
- Do not delete the webview engine until the native one is at parity; it is the working daily driver.
