//! The playback engine: Pandora's protocol plus audio output, sequenced into a working radio.
//!
//! This is what replaces Jarlid's engine webview. It owns the session, keeps a queue of upcoming
//! tracks topped up, plays them in order, and reports what is happening so a UI can render it.
//!
//! Threading: the audio device lives on its own thread ([`audio_thread`]) because `cpal::Stream`
//! is not `Send` on Windows. Everything here is async and talks to it by message.

mod audio_thread;
pub mod credentials;

use std::sync::Arc;
use std::time::{Duration, Instant};

use audio_thread::{AudioThread, Start};
use pandora::Track;
use tokio::sync::{mpsc, Mutex};

/// Re-exported so a caller can pick an output device without taking a direct dependency on
/// the audio crate — the engine is meant to be the whole audio-facing API for the app.
pub use audio::{output_devices, Output};

/// Keep at least this many tracks queued; fetch more when we drop below it. Pandora returns
/// roughly four per request, so this refills well before the queue runs dry.
const MIN_QUEUED: usize = 2;

/// How much of the next track to pull into memory before a blend.
///
/// Only the overlap is played from here; the rest arrives live once the handover completes. More
/// than enough for the longest overlap the settings allow, with room for the measurement the
/// tempo tracker needs before it will say anything.
const PREFETCH: Duration = Duration::from_secs(45);

/// How far ahead of the blend to start preparing it. The fetch itself is well under a second —
/// measured at ~120x realtime — so this is slack, not a budget.
const PREPARE_LEAD: Duration = Duration::from_secs(15);

/// How one track gives way to the next.
///
/// Mirrors the app's own settings rather than sharing them: the app depends on this crate, not
/// the other way round, and a playback engine should not need a Tauri config file to know how to
/// end a song.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlendConfig {
    /// Overlap the tracks at all.
    pub enabled: bool,
    /// Pull the incoming track's tempo onto the outgoing one's. When they are further apart than
    /// `max_pull` allows, there is no blend — a normal transition beats a bad mix.
    pub beat_match: bool,
    pub seconds: f32,
    /// Permitted tempo pull, as a fraction. A DJ pitch-fader range.
    pub max_pull: f32,
}

/// Off. Blending changes how every song ends, so it is asked for rather than assumed.
impl Default for BlendConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            beat_match: true,
            seconds: 8.0,
            max_pull: 0.06,
        }
    }
}

impl BlendConfig {
    /// The rate to play `incoming` at so its beats line up with `outgoing`, or `None` if that
    /// cannot be done within the permitted pull.
    ///
    /// Half and double time count as matched: a 64 BPM track already lines up with a 128 BPM one,
    /// so a DJ would mix them without touching either deck. Candidates are `outgoing · 2^k /
    /// incoming` for k in -1, 0, 1, nearest to 1.0 winning. Doubling the *rate* is never the
    /// answer — that raises the pitch an octave.
    pub fn rate_for(&self, outgoing: Option<f32>, incoming: Option<f32>) -> Option<f64> {
        if !self.beat_match {
            // Crossfade only: play the incoming track at its own speed.
            return Some(1.0);
        }
        let (Some(out), Some(inc)) = (outgoing, incoming) else {
            return None;
        };
        if out <= 0.0 || inc <= 0.0 {
            return None;
        }
        [0.5f32, 1.0, 2.0]
            .into_iter()
            .map(|octave| out * octave / inc)
            .filter(|rate| (rate - 1.0).abs() <= self.max_pull)
            .min_by(|a, b| (a - 1.0).abs().total_cmp(&(b - 1.0).abs()))
            .map(|r| r as f64)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Pandora(#[from] pandora::Error),

    #[error("no station selected")]
    NoStation,

    #[error(transparent)]
    Credentials(#[from] credentials::Error),

    #[error("not signed in")]
    NotSignedIn,
}

impl Error {
    /// Another device holds the account's single permitted stream.
    ///
    /// Delegates to the protocol error so callers don't have to reach through the variant. This
    /// one is recoverable — the engine keeps retrying — so it should never be reported the way a
    /// genuine failure is.
    pub fn is_stream_violation(&self) -> bool {
        matches!(self, Error::Pandora(e) if e.is_stream_violation())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// What the engine is doing, for a UI to render.
#[derive(Debug, Clone)]
pub enum Event {
    TrackStarted(Box<Track>),
    TrackEnded,
    Paused(bool),
    StationChanged(String),
    /// The station's Mode changed ("Crowd Faves", "Deep Cuts", …).
    ModeChanged(String),
    /// Pandora permits one concurrent stream per account; another device has it.
    StreamTaken,
    Error(String),
}

/// A track and when Pandora issued it.
///
/// The audio URL is signed and expires, and a playlist request signs every track in it at once.
/// So when one of them will not open, the others fetched with it are dead too — and the ones
/// fetched later may not be. The instant is what tells those apart; see
/// [`Engine::retire_batch_of_current`].
#[derive(Clone)]
struct Fetched {
    track: Track,
    at: Instant,
}

struct State {
    client: pandora::Client,
    station_token: Option<String>,
    /// The station's **REST** id, which the Modes endpoints need. Resolved on first use and
    /// cached; see [`Engine::rest_station_id`] for why it isn't simply the tuner token.
    station_rest_id: Option<String>,
    station_name: String,
    queue: Vec<Fetched>,
    current: Option<Fetched>,
    /// stationId → station name, so a QuickMix track can name its source without a round trip.
    station_names: std::collections::HashMap<String, String>,
}

pub struct Engine {
    state: Arc<Mutex<State>>,
    audio: Arc<AudioThread>,
    events: mpsc::UnboundedSender<Event>,
    /// How to end a song. Read every tick of `run`, so a plain lock rather than the async one.
    blend: std::sync::Mutex<BlendConfig>,
}

impl Engine {
    /// Log in and start the engine. Returns the engine and a stream of [`Event`]s.
    pub async fn start(
        username: &str,
        password: &str,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Event>)> {
        let client = pandora::Client::login(username, password).await?;
        let (events, receiver) = mpsc::unbounded_channel();

        let engine = Self {
            state: Arc::new(Mutex::new(State {
                client,
                station_token: None,
                station_rest_id: None,
                station_name: String::new(),
                queue: Vec::new(),
                current: None,
                station_names: std::collections::HashMap::new(),
            })),
            audio: Arc::new(AudioThread::spawn()),
            events,
            blend: std::sync::Mutex::new(BlendConfig::default()),
        };

        Ok((engine, receiver))
    }

    /// Start using credentials saved in the Windows Credential Manager.
    ///
    /// Returns [`Error::NotSignedIn`] when nothing is stored, which the app should treat as
    /// "show the login form" rather than as a failure.
    pub async fn start_from_saved() -> Result<(Self, mpsc::UnboundedReceiver<Event>)> {
        let saved = credentials::load()?.ok_or(Error::NotSignedIn)?;
        Self::start(&saved.username, &saved.password).await
    }

    /// Verify credentials by logging in, and only save them if they actually work — so a typo
    /// can't be persisted and then fail mysteriously on every later launch.
    pub async fn sign_in(
        username: &str,
        password: &str,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Event>)> {
        let started = Self::start(username, password).await?;
        credentials::store(username, password)?;
        Ok(started)
    }

    /// Every station, with the playback token and Pandora's special-station flags.
    ///
    /// One call, one type. This was briefly two methods over the same `user.getStationList`
    /// response — one projecting name/token tuples, one parsing the full struct — which parsed
    /// the same payload twice and made every call site pick a lossy or non-lossy variant.
    pub async fn station_list(&self) -> Result<Vec<pandora::TunerStation>> {
        let mut state = self.state.lock().await;
        let stations = state.client.station_list().await?;
        // Cache the id→name map while we have it, so a QuickMix track can name its source
        // station without an extra round trip.
        state.station_names = stations
            .iter()
            .map(|s| (s.station_id.clone(), s.station_name.clone()))
            .collect();
        Ok(stations)
    }

    /// A station's seeds and full thumb history, for export.
    ///
    /// Holds the client lock for one call, same as every other method here; callers are expected
    /// to walk stations one at a time rather than concurrently, which is also what keeps the
    /// request rate polite.
    pub async fn station_details(&self, token: &str) -> Result<serde_json::Value> {
        Ok(self
            .state
            .lock()
            .await
            .client
            .station_details(token)
            .await?)
    }

    /// Which station produced the current track.
    ///
    /// On an ordinary station this is just that station and isn't worth showing. On QuickMix it
    /// answers "what am I actually listening to right now?", which is otherwise invisible.
    /// Returns `None` when it matches the selected station, so callers can render it
    /// unconditionally without special-casing.
    pub async fn source_station(&self) -> Option<String> {
        let mut state = self.state.lock().await;
        let track = state.current.as_ref()?.track.clone();
        if track.station_id.is_empty() {
            return None;
        }

        // Populate the map lazily — the first QuickMix track usually arrives before anything has
        // asked for the station list.
        if state.station_names.is_empty() {
            if let Ok(stations) = state.client.station_list().await {
                state.station_names = stations
                    .iter()
                    .map(|s| (s.station_id.clone(), s.station_name.clone()))
                    .collect();
            }
        }

        let name = state.station_names.get(&track.station_id)?.clone();
        (name != state.station_name).then_some(name)
    }

    /// Modes available for the station currently playing.
    pub async fn modes(&self) -> Result<Vec<pandora::Mode>> {
        let mut state = self.state.lock().await;
        let id = Self::rest_station_id(&mut state).await?;
        Ok(state.client.station_modes(&id).await?)
    }

    /// Resolve (and cache) the REST id for the current station.
    ///
    /// On the test account the tuner `stationToken` and the REST `stationId` are the same value
    /// for every one of the 88 REST stations — but that is an observation, not a guarantee, so we
    /// confirm the id actually exists rather than assuming.
    ///
    /// Name matching is only a fallback, and a weak one: station names are **not unique** (this
    /// account has two both called "Sandstorm Radio"). So a name lookup is used only when exactly
    /// one station carries that name; an ambiguous name is treated as unresolvable rather than
    /// silently picking the wrong station's modes.
    async fn rest_station_id(state: &mut State) -> Result<String> {
        if let Some(id) = &state.station_rest_id {
            return Ok(id.clone());
        }
        // Nothing is playing yet, so there is no station to resolve.
        let token = state.station_token.clone().ok_or(Error::NoStation)?;

        let stations = state.client.stations().await?;

        // Exact: the tuner token is itself a valid REST station id. Unambiguous, so prefer it.
        let id = if stations.iter().any(|s| s.station_id == token) {
            token
        } else {
            let name = state.station_name.clone();
            let mut matches = stations.iter().filter(|s| s.name == name);
            match (matches.next(), matches.next()) {
                (Some(station), None) => station.station_id.clone(),
                // Ambiguous or absent: refuse rather than guess.
                _ => return Err(Error::NoStation),
            }
        };

        state.station_rest_id = Some(id.clone());
        Ok(id)
    }

    /// Switch the current station's Mode.
    ///
    /// Pandora applies this to newly generated playlists, so the queue we already hold is stale.
    /// We drop it and refill, otherwise the change wouldn't be audible for several tracks and
    /// would look broken. The *currently playing* track is deliberately left alone — cutting off
    /// a song mid-play to honour a mode change is more jarring than useful.
    pub async fn set_mode(&self, mode_id: i64) -> Result<()> {
        let name = {
            let mut state = self.state.lock().await;
            let id = Self::rest_station_id(&mut state).await?;
            state.client.set_station_mode(&id, mode_id).await?;
            state.queue.clear();

            state
                .client
                .station_modes(&id)
                .await
                .ok()
                .and_then(|modes| {
                    modes
                        .into_iter()
                        .find(|m| m.mode_id == mode_id)
                        .map(|m| m.mode_name)
                })
                .unwrap_or_default()
        };

        let _ = self.events.send(Event::ModeChanged(name));
        Ok(())
    }

    /// Stop playback and release the audio device.
    ///
    /// For shutdown paths that need the device let go deliberately rather than yanked when
    /// the process exits — notably the updater handover, which replaces the running binary.
    pub fn stop_audio(&self) {
        self.audio.stop();
    }

    /// Switch station and begin playing it.
    pub async fn play_station(&self, name: &str, token: &str) -> Result<()> {
        self.play_station_as(name, token, Start::Playing).await
    }

    async fn play_station_as(&self, name: &str, token: &str, start: Start) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            state.station_token = Some(token.to_string());
            // Different station, so the cached REST id no longer applies.
            state.station_rest_id = None;
            state.station_name = name.to_string();
            state.queue.clear();
            state.current = None;
        }
        self.audio.stop();
        let _ = self.events.send(Event::StationChanged(name.to_string()));
        self.advance_as(start).await
    }

    /// Play the next track, refilling the queue if needed.
    ///
    /// This is the button-press form: the next track starts sounding whatever the pause state
    /// was, which is what skip, take-over and picking a station all mean. The advances the run
    /// loop makes on its own go through [`Engine::advance_as`] with [`Start::Unchanged`].
    pub async fn advance(&self) -> Result<()> {
        self.advance_as(Start::Playing).await
    }

    async fn advance_as(&self, start: Start) -> Result<()> {
        let track = {
            let mut state = self.state.lock().await;

            if state.queue.len() < MIN_QUEUED {
                let token = state.station_token.clone().ok_or(Error::NoStation)?;
                match state.client.playlist(&token).await {
                    Ok(tracks) => {
                        // One stamp for the batch, taken once. The request signed every URL
                        // in it at the same instant, and the retirement rule compares stamps
                        // for equality-or-older: stamping each track as it is pushed would
                        // spread a batch across nanoseconds and let its later members survive.
                        let at = Instant::now();
                        state
                            .queue
                            .extend(tracks.into_iter().map(|track| Fetched { track, at }));
                    }
                    Err(e) if e.is_stream_violation() => {
                        let _ = self.events.send(Event::StreamTaken);
                        return Err(e.into());
                    }
                    // A refill failure is only fatal if we have nothing left to play.
                    Err(e) if state.queue.is_empty() => {
                        let _ = self.events.send(Event::Error(e.to_string()));
                        return Err(e.into());
                    }
                    Err(_) => {}
                }
            }

            if state.queue.is_empty() {
                return Err(Error::NoStation);
            }
            let next = state.queue.remove(0);
            state.current = Some(next.clone());
            next.track
        };

        self.audio.play(&track.audio_url, start);
        let _ = self.events.send(Event::TrackStarted(Box::new(track)));

        // A `TrackStarted` that did not start anything would otherwise leave the event stream
        // implying playback. Jarlid's UI happens not to need it — it takes the truth from the
        // playhead tick — but the event stream is this crate's API and should not lie to the
        // next consumer. For `Unchanged` the published flag is read after the fact: exact
        // unless a pause is still in flight to the audio thread, and the tick corrects that
        // within a frame.
        let paused = match start {
            Start::Playing => false,
            Start::Paused => true,
            Start::Unchanged => self.audio.is_paused(),
        };
        if paused {
            let _ = self.events.send(Event::Paused(true));
        }
        Ok(())
    }

    /// The current track would not play. Retire everything queued that is no fresher than it.
    ///
    /// Pandora's audio URLs are signed and expire, and a playlist request signs every track in
    /// it at once. So a URL that will not open is not one bad track: everything fetched with it
    /// or before it was signed no later, and is dead too. Left in the queue, each would be tried
    /// in turn and fail in turn — which is exactly what "press play after a long pause and it
    /// skips through three songs" was. Anything fetched later may still be good and is kept;
    /// the refill in [`Engine::advance_as`] covers whatever is missing.
    ///
    /// Also reached when a track stalled mid-play and recovery gave up on it, where the rest of
    /// its batch is probably fine. The cost of being wrong there is one playlist request; the
    /// cost of being wrong the other way is the skipping this exists to stop.
    async fn retire_batch_of_current(&self) {
        let mut state = self.state.lock().await;
        let Some(failed_at) = state.current.as_ref().map(|c| c.at) else {
            return;
        };
        retire_no_fresher_than(&mut state.queue, failed_at);
    }

    /// Restore a station without starting it: the track is loaded, the device is never
    /// opened, and pressing play begins it.
    ///
    /// This exists for the updater. An update that installs itself while the listener has
    /// the music paused must not come back playing, or the restart starts music at someone
    /// who deliberately stopped it.
    ///
    /// The intent travels with the one advance it is for, which is the whole reason it is an
    /// argument rather than a flag callers set. A standing flag would be a trap: if this
    /// restore fails — a stream violation moments after the installer ran is the obvious
    /// way — the flag would sit there waiting for the *next* advance, and that one is the
    /// advance the listener asks for by clicking Take Over or picking a station. They would
    /// get a loaded track and silence, with nothing on screen to explain it.
    pub async fn play_station_paused(&self, name: &str, token: &str) -> Result<()> {
        self.play_station_as(name, token, Start::Paused).await
    }

    /// Drive the radio: advance when a track ends. Runs until the engine is dropped.
    ///
    /// Kept as an explicit call rather than a hidden background task so a host app can own its
    /// own loop and interleave its own work.
    /// Choose how one song gives way to the next. Takes effect at the next transition.
    pub fn set_blend(&self, blend: BlendConfig) {
        if let Ok(mut slot) = self.blend.lock() {
            *slot = blend;
        }
    }

    fn blend_config(&self) -> BlendConfig {
        self.blend.lock().map(|b| *b).unwrap_or_default()
    }

    /// Pull the start of the next track into memory and decide whether it can be blended into.
    ///
    /// Decoding is far faster than playback when nothing throttles it — measured at ~120x — so
    /// this costs well under a second and the second connection is open for about that long.
    /// Runs on a blocking thread: the decode would otherwise stall the async runtime, and it must
    /// never go near the audio thread.
    async fn prepare_blend(&self, blend: &BlendConfig) -> Option<(String, Arc<Vec<i16>>, f64)> {
        let format = self.audio.output_format()?;
        let outgoing = self.audio.tempo().map(|t| t.bpm);
        let url = {
            let state = self.state.lock().await;
            state.queue.first()?.track.audio_url.clone()
        };

        let fetch_url = url.clone();
        let fetched =
            tokio::task::spawn_blocking(move || audio::prefetch(&fetch_url, format, PREFETCH))
                .await
                .ok()?
                .ok()?;

        // The decision, made once. No match means no blend at all — a normal transition beats a
        // bad mix, and this is where that is decided rather than halfway through a fade.
        //
        // Both tempos are logged because "declined" has two very different causes that look
        // identical from outside: two songs genuinely too far apart, which is the common and
        // correct answer, and a track whose tempo we never established, which is not.
        let incoming = fetched.tempo.map(|t| t.bpm);
        let rate = blend.rate_for(outgoing, incoming);
        let show = |bpm: Option<f32>| match bpm {
            Some(b) => format!("{b:.1}"),
            None => "unknown".into(),
        };
        eprintln!(
            "[blend] {} BPM -> {} BPM: {}",
            show(outgoing),
            show(incoming),
            match rate {
                Some(r) => format!("rate {r:.4} ({:+.1}%)", (r - 1.0) * 100.0),
                None => "no match".into(),
            }
        );
        Some((url, Arc::new(fetched.pcm), rate?))
    }

    pub async fn run(&self) {
        // How long to wait before trying again after a failed advance. Long enough not to hammer
        // Pandora while another device holds the stream, short enough that playback resumes on
        // its own once that device stops.
        const RETRY: Duration = Duration::from_secs(10);
        let mut retry_at: Option<tokio::time::Instant> = None;

        // What the next transition will use, once prepared. Cleared whenever the track changes,
        // so a skip can never blend into a song that is no longer next.
        let mut prepared: Option<(String, Arc<Vec<i16>>, f64)> = None;
        // Which track we have already *tried* to prepare for, whether or not it produced a blend.
        //
        // Separate from `prepared` on purpose. Deciding not to blend is a perfectly ordinary
        // outcome — most pairs of songs are nowhere near each other's tempo — and keying the
        // retry off `prepared.is_none()` meant every decline was retried on the next tick, five
        // times a second, for the whole approach to the end of the track. That is a hundred-odd
        // pointless downloads of the next song per transition, against an account Pandora only
        // permits one stream on.
        let mut prepared_for: Option<String> = None;

        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;

            // A blend that has handed over is a track change that never went through `advance`:
            // the audio thread swapped the stream underneath us, so the queue has to catch up.
            if self.audio.take_blend_completed() {
                let started = {
                    let mut state = self.state.lock().await;
                    if state.queue.is_empty() {
                        None
                    } else {
                        let next = state.queue.remove(0);
                        state.current = Some(next.clone());
                        Some(next.track)
                    }
                };
                prepared = None;
                prepared_for = None;
                if let Some(track) = started {
                    let _ = self.events.send(Event::TrackStarted(Box::new(track)));
                }
            }

            let blend = self.blend_config();
            if blend.enabled && !self.audio.blending() {
                let position = self.audio.position();
                let duration = self
                    .state
                    .lock()
                    .await
                    .current
                    .as_ref()
                    .map(|c| Duration::from_secs(c.track.track_length))
                    .unwrap_or_default();
                let remaining = duration.saturating_sub(position);
                let overlap = Duration::from_secs_f32(blend.seconds.clamp(2.0, 20.0));
                let current_url = self
                    .state
                    .lock()
                    .await
                    .current
                    .as_ref()
                    .map(|c| c.track.audio_url.clone());

                // Anything prepared for a track we are no longer playing is stale — a skip, or a
                // station change. Throwing it away is cheaper than blending into the wrong song.
                if prepared.is_some() && prepared_for != current_url {
                    prepared = None;
                }

                if !duration.is_zero() && current_url.is_some() {
                    if prepared_for != current_url && remaining <= overlap + PREPARE_LEAD {
                        // Once per track, decision included.
                        prepared_for = current_url.clone();
                        prepared = self.prepare_blend(&blend).await;
                    }
                    if remaining <= overlap {
                        if let Some((url, pcm, rate)) = prepared.take() {
                            // Worth saying out loud: a blend is inaudible when it works, so
                            // without this there is no way to tell "no blend was wanted" from
                            // "a blend was attempted and went wrong".
                            eprintln!(
                                "[blend] starting: {:.1}s overlap at rate {rate:.4} ({:+.1}%)",
                                overlap.as_secs_f32(),
                                (rate - 1.0) * 100.0
                            );
                            self.audio.start_blend(url, pcm, rate, overlap);
                        }
                    }
                }
            }

            let due = retry_at.is_some_and(|at| tokio::time::Instant::now() >= at);
            // Both consumed here, so a failed advance is retried when `RETRY` says, not on
            // every tick that still sees the flag up.
            let ended = self.audio.take_track_ended();
            // A track that failed to open must be skipped, or the radio stalls forever on it.
            let failed = self.audio.take_failed();

            if due || ended || failed {
                if ended || failed {
                    let _ = self.events.send(Event::TrackEnded);
                }
                if failed {
                    self.retire_batch_of_current().await;
                }

                // Nobody pressed anything, so the pause state is not ours to change. This is
                // what keeps "play, then pause a moment later" paused: if the track play tried
                // to open was dead, its replacement loads the way the listener left things
                // rather than starting because an advance happened to run.
                match self.advance_as(Start::Unchanged).await {
                    Ok(()) => retry_at = None,
                    Err(e) => {
                        // Do NOT give up. This used to `return`, so a single STREAM_VIOLATION
                        // killed auto-advance for the rest of the session — the radio stayed dead
                        // even after the other device stopped playing. `advance` already emitted
                        // the specific event, so just schedule another attempt.
                        if !e.is_stream_violation() {
                            let _ = self.events.send(Event::Error(e.to_string()));
                        }
                        retry_at = Some(tokio::time::Instant::now() + RETRY);
                    }
                }
            }
        }
    }

    /// Claim the account's single stream for this device.
    ///
    /// Pandora exposes no explicit "take over" call; a client claims the stream simply by asking
    /// for a playlist and being the most recent to do so. So this is a retry — but a retry is
    /// exactly what the native app's takeover amounts to.
    ///
    /// Whether the other device is actively evicted or merely loses the next request is not
    /// something we can observe from here.
    pub async fn take_over(&self) -> Result<()> {
        self.advance().await
    }

    pub async fn skip(&self) -> Result<()> {
        self.audio.stop();
        self.advance().await
    }

    /// Restart the current track from the beginning.
    ///
    /// No seek required and no extra request to Pandora: the signed audio URL is still valid, so
    /// re-opening it starts a fresh decode from byte zero.
    pub async fn replay(&self) -> Result<()> {
        let Some(track) = self.now_playing().await else {
            return Err(Error::NoStation);
        };
        self.audio.stop();
        self.audio.play(&track.audio_url, Start::Playing);
        // Re-announce so the UI resets its progress bar and re-syncs lyrics to zero.
        let _ = self.events.send(Event::TrackStarted(Box::new(track)));
        Ok(())
    }

    /// Move the playhead within the current track.
    ///
    /// Deliberately does *not* announce a track start the way [`Engine::replay`] does: the
    /// playhead ticker publishes the new position within a frame or two, and re-announcing would
    /// tell the UI to reset its progress bar and reload lyrics for a track that never changed.
    pub fn seek(&self, to: Duration) {
        self.audio.seek(to);
    }

    pub fn set_paused(&self, paused: bool) {
        self.audio.set_paused(paused);
        let _ = self.events.send(Event::Paused(paused));
    }

    pub fn toggle_pause(&self) {
        self.set_paused(!self.audio.is_paused());
    }

    pub fn set_volume(&self, volume: f32) {
        self.audio.set_volume(volume);
    }

    /// Choose which output endpoint to play on. Moves the current song there rather than
    /// waiting for the next one.
    pub fn set_output(&self, output: audio::Output) {
        self.audio.set_output(output);
    }

    /// The endpoint audio is actually going to, or `None` when nothing is open. Distinct from
    /// what was *chosen* — an absent device falls back to the default.
    pub fn output_device(&self) -> Option<String> {
        self.audio.output_device()
    }

    /// Where the listener actually is in the current track — the right clock for synced lyrics.
    pub fn position(&self) -> Duration {
        self.audio.position()
    }

    pub fn buffered(&self) -> Duration {
        self.audio.buffered()
    }

    /// Lost-audio detector. Should stay at zero; anything else means dropped samples.
    pub fn drift(&self) -> Duration {
        self.audio.drift()
    }

    /// Total silence played because decoding fell behind — the audible dropout, measured.
    /// Should stay at zero; anything else means the decode thread is losing CPU.
    pub fn starved(&self) -> Duration {
        self.audio.starved()
    }

    /// The tempo of the current track, measured from its audio.
    ///
    /// Pandora sends no BPM — nor key, nor anything else musicological — so this is computed
    /// from the decoded samples as they play. `None` until roughly ten seconds in, because
    /// decoding is throttled to about playback speed and there is genuinely no more to go on.
    pub fn tempo(&self) -> Option<audio::Tempo> {
        self.audio.tempo()
    }

    /// What is actually being decoded: codec, bitrate and the source's own sample rate, read
    /// from the container. Deliberately not [`pandora::Track::audio_encoding`], which describes
    /// the default stream rather than the better one we asked for and were granted.
    pub fn source(&self) -> Option<audio::Source> {
        self.audio.source()
    }

    /// The rate audio is decoded to, which is the output device's own. Pandora sends 44.1 kHz
    /// and most Windows endpoints run at 48; the difference is real resampling, and playing one
    /// as the other would run everything about 8.8% sharp.
    pub fn output_rate(&self) -> Option<u32> {
        self.audio.output_rate()
    }

    pub fn is_paused(&self) -> bool {
        self.audio.is_paused()
    }

    pub async fn now_playing(&self) -> Option<Track> {
        self.state
            .lock()
            .await
            .current
            .as_ref()
            .map(|c| c.track.clone())
    }

    pub async fn thumb_up(&self) -> Result<()> {
        self.feedback(true).await
    }

    pub async fn thumb_down(&self) -> Result<()> {
        self.feedback(false).await
    }

    async fn feedback(&self, positive: bool) -> Result<()> {
        let mut state = self.state.lock().await;
        let (Some(station), Some(track)) = (
            state.station_token.clone(),
            state.current.as_ref().map(|c| c.track.clone()),
        ) else {
            return Err(Error::NoStation);
        };

        if positive {
            state.client.thumb_up(&station, &track.track_token).await?;
        } else {
            state
                .client
                .thumb_down(&station, &track.track_token)
                .await?;
        }
        Ok(())
    }
}

/// Drop every queued track fetched at or before `at`, keeping anything fetched later.
///
/// Separate from [`Engine`] so the rule can be tested without a Pandora session: tracks from
/// one playlist request share one stamp exactly, and the comparison has to treat equal stamps
/// as the same batch or the retirement would keep the very tracks it exists to drop.
fn retire_no_fresher_than(queue: &mut Vec<Fetched>, at: Instant) {
    queue.retain(|queued| queued.at > at);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetched(title: &str, at: Instant) -> Fetched {
        Fetched {
            track: Track {
                song_title: title.into(),
                ..Track::default()
            },
            at,
        }
    }

    fn titles(queue: &[Fetched]) -> Vec<&str> {
        queue.iter().map(|q| q.track.song_title.as_str()).collect()
    }

    /// The failed track's batch goes; a batch fetched later survives, in order.
    #[test]
    fn retires_the_failed_batch_and_keeps_the_later_one() {
        let stale = Instant::now();
        let fresh = stale + Duration::from_secs(1);
        let mut queue = vec![
            fetched("stale 1", stale),
            fetched("stale 2", stale),
            fetched("fresh 1", fresh),
            fetched("fresh 2", fresh),
        ];

        retire_no_fresher_than(&mut queue, stale);

        assert_eq!(titles(&queue), ["fresh 1", "fresh 2"]);
    }

    /// Equal stamps are the same batch — the failed track was stamped with these.
    #[test]
    fn treats_an_equal_stamp_as_the_same_batch() {
        let at = Instant::now();
        let mut queue = vec![fetched("a", at), fetched("b", at)];

        retire_no_fresher_than(&mut queue, at);

        assert!(queue.is_empty());
    }

    /// A batch older than the failed one is at least as dead.
    #[test]
    fn retires_batches_older_than_the_failed_one() {
        let older = Instant::now();
        let failed = older + Duration::from_secs(1);
        let mut queue = vec![fetched("older", older), fetched("same", failed)];

        retire_no_fresher_than(&mut queue, failed);

        assert!(queue.is_empty());
    }
}
