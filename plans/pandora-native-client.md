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

**Architecture unblocked — option A CONFIRMED. The client can be fully native with no browser.**
Protocol, audio decode, streaming and auth are all proven. One measurement outstanding (REST audio
bitrate for the paid tier), blocked only by Jarlid holding the account's single stream.

### 🎉 2026-08-07 THE BIG QUESTION IS ANSWERED

A `userAuthToken` obtained from the **tuner** API **is accepted** by the modern web REST API as
`X-AuthToken`. The 2021 claim was right, and it still holds in 2026.

```
=== 6. THE BIG ONE — tuner token against the web REST API ===
*** ACCEPTED *** — REST returned 5 stations.
```

**Consequence: no browser anywhere in the client.** Log in over the tuner API (Blowfish, no bot
wall), then use the rich 135-endpoint REST surface with that token. Option C (one-time browser
login) is no longer needed, and option D was never worth it.

## Decisions (locked 2026-08-06/07)

1. **Auth path: tuner login, then REST for everything else** (option A).
   ✅ **CONFIRMED 2026-08-07** — the tuner `userAuthToken` IS accepted as REST `X-AuthToken`.
   Fully browser-free. Tuner is used for **login only**; all other calls (including audio, which
   the tuner API caps at 64 kbps) go over REST. Options C and D are dead.
2. **Scope: swap the engine, keep Jarlid.** The webview engine is one replaceable module.
   Lyrics sync/cache, SMTC, taskbar thumb toolbar, UPnP/WiiM remote, auto-update and window state
   are all protocol-agnostic and stay.
3. **Audio decode: Windows Media Foundation.** Decodes HE-AAC (incl. SBR/PS) natively with no
   extra shipped dependency; the app is already Windows-only and already uses `windows-rs` for
   SMTC and the thumbbar. Symphonia is rejected — no SBR/PS, would decode dull and half-bandwidth.
   ✅ **Confirmed empirically 2026-08-07** — the stream really is HE-AAC with SBR (see findings).

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

1. [x] **Spike — auth path.** Tuner `auth.userLogin` works; its token IS accepted by REST.
   Option A confirmed, no browser needed.
2. [x] **Spike — audio.** HE-AAC confirmed (SBR required); Media Foundation decodes it to
   44.1 kHz stereo PCM, and streams the HTTPS URL directly with no temp file.
3. [x] **Audio quality measured.** 128 kbps MP3 via tuner `additionalAudioUrl` — double the
   64 kbps default. REST playback turned out to be refused on a tuner token entirely.
4. [x] **Typed client built and verified read-only** — `pandora::Client` (tuner login → REST),
   `models::{Station, Track, Art}`, paginated `stations()`, `fragment()`, `search()`, and silent
   re-login on token expiry. `cargo run --example list-stations` returns 88 stations with 0
   missing names or ids. Write paths (thumbs, tired, trackStarted) are implemented but
   **deliberately unexecuted** — see below.
5. [x] **Player built — music plays through the speakers.** `audio::Player` with a decode thread,
   ring buffer, cpal output, pause and volume. Position is honest (see below).
   Remaining for this step: prefetch the *next* track for gapless transitions, and sequence a
   whole station rather than one track.
6. [ ] Define a `PandoraEngine` trait; implement `NativeEngine`; keep `WebviewEngine` until parity.
7. [ ] Wire existing UI/SMTC/thumbbar/lyrics to the native engine (they consume events, not DOM).
8. [ ] Send the telemetry endpoints (`trackStarted`, `event/*`, `audioReceiptURL`) so our traffic
   looks like a real client.
9. [ ] Delete the engine webview and `bridge.js`. Rewrite the README — its "Why this approach"
   section currently argues *for* the wrapper and is now wrong, as is the DRM sentence in the
   disclaimer.

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

### 2026-08-07 Audio decision CLOSED — HE-AAC confirmed, Symphonia definitively ruled out

`cargo run --bin audio-probe` runs the whole chain on the **anonymous** tier (no account touched):
anonymous login → search → createStation → getFragment → real signed audio URL → Range-fetch the
container → parse the AudioSpecificConfig out of `moov/…/stsd/mp4a/esds`.

Result on a real Pandora stream (Pink Floyd station, `audioEncoding: "aacplus"`):

```
codec:        HE-AAC (AAC-LC + SBR)
output rate:  44100 Hz
channels:     2
core rate:    22050 Hz (SBR doubles it to 44100)
boxes: ftyp moov mvhd trak tkhd edts elst mdia mdhd hdlr minf smhd dinf dref stbl
       stsd mp4a esds btrt stts stsc stsz stco sgpd sbgp udta meta hdlr ilst free mdat
```

- **SBR is required.** Audio Object Type 5. Symphonia would decode the core layer only — 22050 Hz
  instead of 44100 Hz, no high band, audibly dull. **Windows Media Foundation confirmed.**
- **No DRM, proven structurally**: not one of `pssh` `sinf` `schm` `tenc` `enca` `encv` appears
  anywhere in the box tree. Plain progressive MP4/AAC, Range requests honoured (`206`).
- **No XOR `key` field** on this tier — the obfuscation path stays dormant. Paid tier still
  `[UNCONFIRMED]`; the credentialed probe checks it.
- The whole flow works with **a plain HTTP client, no browser, no bot-detection trouble** —
  strong evidence the REST surface is fully reachable natively once we hold a token.

**API corrections found the hard way** (the 2021 public docs are wrong here):
- `station/createStation` wants **`pandoraId`** (e.g. `AR:105740`). The documented `stationCode`
  field is rejected with `GENERIC`; `musicId` with `INVALID_REQUEST`.
- `search/fullSearch` **ignores the `types` filter** — it returns composers/genres/tracks mixed in
  regardless, so filter client-side on `type == "artist"` or you'll seed a station with a composer.
- Audio hosts vary per request (`t1-4.p-cdn.us`, `audio-usc-mp1-t1-1-v4v6.pandora.com`) — don't
  pin a hostname.
- `moov` was at the front here, but the probe falls back to fetching the file tail, since
  non-streaming-optimised MP4s put it last. Keep that fallback.

### 2026-08-07 Audio path CONFIRMED end to end — Media Foundation decodes it correctly

`cargo run --example decode-probe` (in `crates/audio`) fetches a real anonymous-tier stream,
downloads it, and decodes it through Media Foundation:

```
track:    Breathe (In the Air) (2023 Remaster) — Pink Floyd
encoding: aacplus
negotiated output: 44100 Hz, 2 ch, 16-bit PCM
decoded 29933568 bytes = 169.7 s of audio
RMS 4496, peak 30347 (of 32767)
OK — real audio, not silence.
OK — 44100 Hz output: SBR WAS applied.
OK — decoded 102% of the reported length.
```

- **44100 Hz out from a 22050 Hz core proves SBR was applied.** This is the empirical confirmation
  that Media Foundation does the job Symphonia cannot — not an assertion from documentation.
- RMS 4496 / peak 30347 confirms real audio rather than silence, which is how a subtly
  misconfigured decode usually presents (correct format, empty buffers).
- 102% of the reported length is expected: decoder priming/padding, not dropped audio.
- `MFCreateSourceReaderFromURL` does demux + decode in one object; asking for `MFAudioFormat_PCM`
  output makes MF insert the AAC decoder itself. No codec code of our own.

**Note for the real player:** the probe writes a temp file because MF wants a URL/path. Production
playback should implement a custom `IMFByteStream` fed from the network so nothing touches disk and
playback can start before the download completes.

### 2026-08-07 Credentialed probe results (paid account)

```
2. tuner auth.userLogin          OK — userAuthToken, 46 chars
3. account tier                  isSubscriber: true, pandoraBrandingType: "p1",
                                 canPurchase: false, canSubscribe: false
4. tuner user.getStationList     OK — 89 stations
5. tuner station.getPlaylist     lowQuality 32 kbps aacplus
                                 mediumQuality 64 kbps aacplus
                                 highQuality 64 kbps aacplus
                                 XOR `key` present: false
6. tuner token on REST           *** ACCEPTED ***
```

- ⚠️ **The tuner API caps audio at 64 kbps even for a paid subscriber.** Pandora advertises up to
  192 kbps on paid web tiers, so **audio must come from the REST `playlist/getFragment`, not the
  tuner `station.getPlaylist`.** Use tuner for *login only*; do everything else over REST. This is
  the single most important architectural consequence of the probe.
- **No XOR `key` on the paid tier either** — the obfuscation path stays dormant, so the decode
  pipeline needs no un-masking step. (Anonymous *and* paid both checked now.)
- `pandoraBrandingType: "p1"` with `isSubscriber: true` confirms an active paid subscription.

### ⚠️ 2026-08-07 CORRECTION — REST gives metadata, but NOT audio, on a tuner token

An earlier entry said "Option A confirmed: fully native, rich REST surface". That was **too
broad**. Refined by `cargo run --example stream-diagnose`, with Jarlid closed and nothing else
streaming:

```
TEST 1  REST getFragment, first call of a fresh session   FAILED: STREAM_VIOLATION
TEST 2  tuner station.getPlaylist, same session           OK — 4 items
TEST 4  anonymous REST session, same endpoint             OK
```

REST playback is refused on the *first* playback call of a fresh tuner session, while tuner
playback on that very session works, and an anonymous *web* session works fine. So **playback is
tied to session type**: a tuner/device token buys REST metadata but not REST audio.

**Final division of labour** (each settled by measurement, not preference):

| Purpose | API | Why |
|---|---|---|
| Login | tuner | No PerimeterX wall; REST `auth/login` 403s any non-browser |
| Station list | REST | Richer — 1080px art and `dominantColor`, absent from the tuner list |
| **Audio** | tuner | REST playback refused; tuner also yields *better* audio (below) |
| Feedback / stations | tuner | Verified end to end |

Still true and still the headline: **no browser anywhere.**

### 🔊 2026-08-07 Audio quality — 128 kbps MP3 is this account's ceiling

`cargo run --example tuner-quality`. Bitrate **measured** (total bytes × 8 ÷ duration), never read
off a label:

```
HTTP_192_MP3             NOT AVAILABLE (field absent)
HTTP_128_MP3             AVAILABLE — measured 127 kbps   <- 2x the default
HTTP_64_AACPLUS_ADTS     AVAILABLE — measured 64 kbps
audioUrlMap high/medium  measured 64 kbps  (the default)
```

- The default `audioUrlMap` caps at 64 kbps, but `additionalAudioUrl: "HTTP_128_MP3"` yields
  **128 kbps MP3 — double the bitrate.** `BEST_AUDIO` in `client.rs`.
- `HTTP_192_MP3` is **not served** to this subscription, despite Pandora advertising up to
  192 kbps. Whether a genuine *web* session would get it is **[UNCONFIRMED]** and would require
  the browser-token path we just eliminated. Open question, deliberately not chased.

⚠️ **Trap: never request multiple specs in one call.** Pandora **drops** unavailable specs from the
returned array instead of leaving empty slots, so the array silently stops lining up with the
request order. Asking for `192,128,64,32` returned three URLs and made every reading look exactly
one step low — 192 appeared to be 128, 128 appeared to be 64. **Request one spec per call.**

MP3 needs no SBR, so this incidentally re-opens Symphonia as a portable option. Not switching:
Media Foundation already handles both formats with zero shipped dependencies.

### ✅ 2026-08-07 Write paths VERIFIED (throwaway station, created and deleted)

User authorised one disposable station. `cargo run --example verify-writes` created it, exercised
every write path, and deleted it:

```
✅ station.createStation      ✅ station.renameStation
✅ station.addFeedback (up)   ✅ station.deleteFeedback
✅ station.addFeedback (down) ✅ user.sleepSong
✅ station.deleteStation
```

**The previously-guessed REST shapes were wrong**, as suspected — the working endpoints are
tuner-side and key off `trackToken`/`stationToken`, not `pandoraId`. `client.rs` now uses the
verified calls. `station.deleteFeedback` needs the `feedbackId` from the add response, which is
how the real client undoes a mis-tap.

### 2026-08-07 Track art — the tuner API needs a size-list synthesised

The tuner API returns a single `albumArtUrl` string, not REST's array of sizes, so tuner-sourced
tracks initially rendered with **no artwork at all** (`art: 0px`). The URL encodes its dimensions
(`…_500W_500H.jpg`) and the CDN serves other sizes from the same path, so `art_sizes_from_url`
rewrites them to offer 130/500/640/1080. **Verified live: the synthesised 1080px URL resolves
(HTTP 206)** — not assumed. Unrecognised URL shapes degrade to the original rather than
fabricating 404s.

### 🔊 2026-08-07 IT PLAYS — `cargo run --example play`

```
station:  QuickMix
encoding: mp3 · 153 s
output: 48000 Hz, 2 ch (started in 242.808ms)
  position   2.5s   buffered  5.0s
  -- pausing for 1s --
  position   3.0s   buffered  5.0s
played 8.4s
=> Music played through the speakers. Native client is audible.
```

Three details that prove the hard parts are right, rather than merely appearing to work:

1. **`output: 48000 Hz`.** The device runs at 48 kHz; Pandora's audio is 44.1 kHz. `Decoder::open_at`
   requests the device's format so **Media Foundation inserts a resampler**. Ignoring this would
   have played everything ~9% sharp — subtly wrong in a way that's easy to ship and unpleasant to
   listen to. No resampling crate needed.
2. **Pause is real.** Across the pause, 1.5 s of wall-clock elapsed but position advanced only
   0.5 s. Position is computed from **frames delivered to the device**, not frames decoded —
   decoding runs ~5 s ahead, so tracking decode progress would run synced lyrics early. This is
   the correct clock for lyric sync.
3. **`buffered` holds at ~5 s.** The decode thread backs off when the buffer is full, so a track
   is streamed rather than pulled wholly into memory.

Architecture: decode thread → `Mutex<VecDeque<i16>>` ring buffer → cpal callback. The callback
never allocates or blocks (`try_lock`, fixed-point volume) because an audio-thread stall is an
audible click. Unfilled output is explicitly zeroed — otherwise the device replays stale memory as
a buzz. `Drop` signals the decode thread to exit so it stops pulling from the network when the
caller moves on.

### ⚠️ 2026-08-07 Pandora enforces ONE concurrent stream per account (`STREAM_VIOLATION`)

REST `playlist/getFragment` fails with `STREAM_VIOLATION` while another client is streaming on the
same account. Confirmed cause: **Jarlid was running** (the existing webview app, PID 15024) and
holding the stream. Not a bug in our code — the identical request succeeds on the anonymous tier.

Consequences for the build:
- The native client and the webview engine **cannot run at the same time**. Plan the cutover
  accordingly, and don't leave both installed and auto-starting.
- Requesting a tuner playlist and then a REST fragment in one session **also** trips it — they
  count as two streams. The probe gained a `--rest-only` flag for exactly this reason.
- `STREAM_VIOLATION` must be handled gracefully in the real client with a clear message
  ("Pandora is playing on another device"), not surfaced as a generic failure.

### 2026-08-07 Streaming gap CLOSED — no custom `IMFByteStream` needed

`cargo run --example stream-probe` (in `crates/audio`):

```
opening the HTTPS URL directly with Media Foundation
OPENED in 170.65ms — 44100 Hz, 2 ch, 16-bit
first PCM chunk: 8192 bytes after 4.54ms
```

Media Foundation's own scheme handlers open Pandora's signed HTTPS URL directly and stream it
progressively — first audio 4.5 ms after opening, so it is not buffering the whole file. **The
"implement a custom IMFByteStream" work item is deleted.** Playback can start ~170 ms after a
track URL is in hand, with no temp file and nothing touching disk.

### 2026-08-07 Typed client — design notes

- **Every model field is `#[serde(default)]` on purpose.** This is an undocumented API we don't
  control. A field Pandora renames should degrade one value, never fail the whole response and
  take the user's music with it. Real-world justification from the live run: 3 of 88 stations have
  no art at all, and only 72 of 88 carry `dominantColor` — a strict model would have thrown.
- `dominantColor` (hex RGB sampled from station art) is a free gift for UI theming — the original
  goal of art-driven visuals, without us analysing images ourselves.
- `TrackKind` exists because fragments interleave `ArtistMessage` and ad items with real music.
  Treating those as songs is how a player ends up showing "Now playing: (untitled)" and looking up
  lyrics for an advert.
- Auth expiry (code 1001) triggers a silent re-login rather than bouncing the user to a sign-in
  screen mid-song.
- Station art is available at **1080px**, which is ample for a full-window hero image.

**⚠️ Write paths are implemented but UNVERIFIED and deliberately never executed.**
`thumb_up` / `thumb_down` / `tired_of_track` / `report_track_started` mutate the real account —
thumbs permanently shape a station's behaviour. The endpoint *names* come from the shipping web
bundle, but the request *bodies* are inferred and the field names are likely wrong (`trackToken`
vs `pandoraId` is the obvious suspect). **Verify each against a throwaway station before trusting
it.** Do not assume they work because they compile.

**Two parser bugs worth remembering** (both fixed, both would have silently produced wrong answers):
1. The box walker bailed entirely when a box ran past a truncated download, so a 32 KiB fetch
   reported only `ftyp` and hid the whole tree.
2. `box_types` recursed into *non-container* boxes, parsing raw media and table data as box names.
   That risked a **false positive on the DRM check** — random bytes can spell `sinf`. Recursion is
   now restricted to a known container list with correct per-box header offsets.
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
