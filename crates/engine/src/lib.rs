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
use std::time::Duration;

use audio_thread::AudioThread;
use pandora::{Station, Track};
use tokio::sync::{mpsc, Mutex};

/// Keep at least this many tracks queued; fetch more when we drop below it. Pandora returns
/// roughly four per request, so this refills well before the queue runs dry.
const MIN_QUEUED: usize = 2;

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

pub type Result<T> = std::result::Result<T, Error>;

/// What the engine is doing, for a UI to render.
#[derive(Debug, Clone)]
pub enum Event {
    TrackStarted(Box<Track>),
    TrackEnded,
    Paused(bool),
    StationChanged(String),
    /// Pandora permits one concurrent stream per account; another device has it.
    StreamTaken,
    Error(String),
}

struct State {
    client: pandora::Client,
    station_token: Option<String>,
    station_name: String,
    queue: Vec<Track>,
    current: Option<Track>,
}

pub struct Engine {
    state: Arc<Mutex<State>>,
    audio: Arc<AudioThread>,
    events: mpsc::UnboundedSender<Event>,
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
                station_name: String::new(),
                queue: Vec::new(),
                current: None,
            })),
            audio: Arc::new(AudioThread::spawn()),
            events,
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

    pub async fn stations(&self) -> Result<Vec<Station>> {
        Ok(self.state.lock().await.client.stations().await?)
    }

    /// Station name/token pairs. The tuner token is what playback needs; the REST station list
    /// does not carry it.
    pub async fn tuner_stations(&self) -> Result<Vec<(String, String)>> {
        Ok(self.state.lock().await.client.tuner_stations().await?)
    }

    /// Switch station and begin playing it.
    pub async fn play_station(&self, name: &str, token: &str) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            state.station_token = Some(token.to_string());
            state.station_name = name.to_string();
            state.queue.clear();
            state.current = None;
        }
        self.audio.stop();
        let _ = self.events.send(Event::StationChanged(name.to_string()));
        self.advance().await
    }

    /// Play the next track, refilling the queue if needed.
    pub async fn advance(&self) -> Result<()> {
        let track = {
            let mut state = self.state.lock().await;

            if state.queue.len() < MIN_QUEUED {
                let token = state.station_token.clone().ok_or(Error::NoStation)?;
                let is_start = state.current.is_none() && state.queue.is_empty();
                match state.client.playlist(&token).await {
                    Ok(tracks) => state.queue.extend(tracks),
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
                let _ = is_start;
            }

            if state.queue.is_empty() {
                return Err(Error::NoStation);
            }
            let track = state.queue.remove(0);
            state.current = Some(track.clone());
            track
        };

        self.audio.play(&track.audio_url);
        let _ = self.events.send(Event::TrackStarted(Box::new(track)));
        Ok(())
    }

    /// Drive the radio: advance when a track ends. Runs until the engine is dropped.
    ///
    /// Kept as an explicit call rather than a hidden background task so a host app can own its
    /// own loop and interleave its own work.
    pub async fn run(&self) {
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;

            // A track that failed to open must be skipped, or the radio stalls forever on it.
            if self.audio.track_ended() || self.audio.failed() {
                let _ = self.events.send(Event::TrackEnded);
                if let Err(e) = self.advance().await {
                    let _ = self.events.send(Event::Error(e.to_string()));
                    return;
                }
            }
        }
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
        let Some(track) = self.state.lock().await.current.clone() else {
            return Err(Error::NoStation);
        };
        self.audio.stop();
        self.audio.play(&track.audio_url);
        // Re-announce so the UI resets its progress bar and re-syncs lyrics to zero.
        let _ = self.events.send(Event::TrackStarted(Box::new(track)));
        Ok(())
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

    pub fn is_paused(&self) -> bool {
        self.audio.is_paused()
    }

    pub async fn now_playing(&self) -> Option<Track> {
        self.state.lock().await.current.clone()
    }

    pub async fn thumb_up(&self) -> Result<()> {
        self.feedback(true).await
    }

    pub async fn thumb_down(&self) -> Result<()> {
        self.feedback(false).await
    }

    async fn feedback(&self, positive: bool) -> Result<()> {
        let mut state = self.state.lock().await;
        let (Some(station), Some(track)) = (state.station_token.clone(), state.current.clone())
        else {
            return Err(Error::NoStation);
        };

        if positive {
            state.client.thumb_up(&station, &track.track_token).await?;
        } else {
            state.client.thumb_down(&station, &track.track_token).await?;
        }
        Ok(())
    }
}
