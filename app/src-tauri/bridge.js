// bridge.js — injected into the Pandora (engine) webview as a Tauri initialization script.
// This is the ONLY code that touches Pandora's DOM. It:
//   1. scrapes now-playing state via Pandora's stable [data-qa=...] hooks,
//   2. sets navigator.mediaSession (Windows SMTC / media keys),
//   3. emits state to the Rust host via the Tauri event system,
//   4. exposes window.__PANDORA_BRIDGE__.cmd(name, arg) for the host to drive playback.
(function () {
  "use strict";
  if (window.__PANDORA_BRIDGE_INSTALLED__) return;
  window.__PANDORA_BRIDGE_INSTALLED__ = true;

  var LOG = function () {
    try {
      var a = ["[bridge]"].concat([].slice.call(arguments));
      console.log.apply(console, a);
    } catch (e) {}
  };
  var injectedAt = Date.now();
  LOG("v3 injected @", location.href, "top-frame:", window.top === window);
  // Only run in the top frame — Pandora has cross-origin iframes we must ignore.
  if (window.top !== window) {
    LOG("skipping sub-frame");
    return;
  }

  // Each entry lists selector candidates in preference order: the big Now
  // Playing page first, then the mini player / tuner bar that exists on every
  // page — so detection keeps working when the engine sits on a deep page.
  var QA = {
    title: ["playing_track_title", "mini_track_title"],
    artist: ["playing_artist_name", "mini_track_artist_name"],
    album: ["playing_album_name"],
    station: ["station_active_name"],
    art: ["album_active_image"],
    artMini: ["mini_track_image"],
    play: ["play_button"],
    pause: ["pause_button"],
    skip: ["skip_button"],
    replay: ["replay_button"],
    up: ["thumbs_up_button"],
    down: ["thumbs_down_button"],
    volume: ["volume_slider"],
  };

  function qa(name) {
    var keys = QA[name];
    for (var i = 0; i < keys.length; i++) {
      var el = document.querySelector('[data-qa="' + keys[i] + '"]');
      if (el) return el;
    }
    return null;
  }
  // Collapse a string that is exactly one part repeated k times ("AbcAbcAbc" →
  // "Abc"). Pandora's marquee clones its content 2-3x while scrolling.
  function derepeat(s) {
    s = (s || "").trim();
    for (var k = 4; k >= 2; k--) {
      if (s.length >= k && s.length % k === 0) {
        var part = s.slice(0, s.length / k);
        var rebuilt = "";
        for (var i = 0; i < k; i++) rebuilt += part;
        if (rebuilt === s) return derepeat(part);
      }
    }
    return s;
  }

  function txt(name) {
    var el = qa(name);
    if (!el) return "";
    // Prefer the marquee's first content node, then collapse any cloned repeats.
    var content = el.querySelector(".Marquee__wrapper__content");
    return derepeat((content || el).textContent || "");
  }
  // Pandora keeps multiple <audio> elements (preloaded next track AND leftover
  // previous tracks after skips). Pick the live one; among paused candidates
  // prefer the NEWEST (later in the DOM) — stale old tracks linger first.
  function audioEl() {
    var els = document.querySelectorAll("audio");
    if (!els.length) return null;
    // 1) the element actually playing right now
    for (var i = 0; i < els.length; i++) {
      if (!els[i].paused && !els[i].ended) return els[i];
    }
    // 2) newest paused-mid-track element
    for (var k = els.length - 1; k >= 0; k--) {
      if (!els[k].ended && els[k].currentTime > 0) return els[k];
    }
    // 3) newest loaded element with a real duration
    for (var j = els.length - 1; j >= 0; j--) {
      if (!els[j].ended && isFinite(els[j].duration) && els[j].duration > 0)
        return els[j];
    }
    return els[els.length - 1];
  }

  function isActuallyPlaying() {
    var els = document.querySelectorAll("audio");
    for (var i = 0; i < els.length; i++) {
      if (!els[i].paused && !els[i].ended) return true;
    }
    return false;
  }

  // Pandora album art URLs embed size as _<w>W_<h>H.jpg — request a larger one for the hero.
  function artUrl(size) {
    var el = qa("art") || qa("artMini");
    var img = el ? (el.tagName === "IMG" ? el : el.querySelector("img")) : null;
    var src = img ? img.src : "";
    if (!src) return "";
    return src.replace(/_(\d+)W_(\d+)H\./, "_" + size + "W_" + size + "H.");
  }

  function isLoginPage() {
    return !!document.querySelector('input[type="password"], input[name="password"]');
  }

  // ---- emit to Rust host -------------------------------------------------
  var warnedNoTauri = false;
  function emit(event, payload) {
    try {
      if (window.__TAURI__ && window.__TAURI__.event) {
        window.__TAURI__.event.emit(event, payload);
      } else if (!warnedNoTauri) {
        warnedNoTauri = true;
        LOG("WARNING: window.__TAURI__ not present — events will not reach the host");
      }
    } catch (e) {
      LOG("emit error", event, e && e.message);
    }
  }

  // ---- now-playing snapshot ---------------------------------------------
  var lastKey = "";
  var lastMeta = { key: "", album: "", station: "" };
  var everHadTitle = false;
  function snapshot() {
    var title = txt("title");
    if (!title) {
      // No player yet: either still loading, or we genuinely need a login.
      // Never signal login once we've seen a real track (transient re-renders),
      // and give the SPA time to redirect "/" to the player before judging —
      // otherwise the engine window flashes up on every cold start.
      if (everHadTitle) return;
      if (Date.now() - injectedAt < 8000) return;
      var loginish =
        isLoginPage() ||
        /account\/sign|login|signin/i.test(location.pathname) ||
        !!document.querySelector('a[href*="/account/sign-in"]');
      if (loginish) emit("engine://needs-login", { needsLogin: true });
      return;
    }
    everHadTitle = true;
    // Title present => the player is up. Never show the login card.
    // On deep pages only the mini player exists (no album/station elements);
    // reuse the last known values for the same track so keys don't churn.
    var artist0 = txt("artist");
    var album0 = txt("album");
    var station0 = txt("station");
    var taKey = title + "|" + artist0;
    if (!album0 && lastMeta.key === taKey) album0 = lastMeta.album;
    if (!station0) station0 = lastMeta.station;
    lastMeta = { key: taKey, album: album0, station: station0 };
    var data = {
      title: title,
      artist: artist0,
      album: album0,
      station: station0,
      art: artUrl(1080),
      artFallback: artUrl(500),
      thumbUp: attrPressed("up"),
      thumbDown: attrPressed("down"),
    };
    // Include art in the key: Pandora updates the art element slightly after the
    // title, so a title-only key would leave stale art on screen.
    collectStations();
    var key = data.title + "|" + data.artist + "|" + data.album + "|" + data.art;
    if (key !== lastKey) {
      lastKey = key;
      emit("engine://nowplaying", data);
    } else {
      // still emit thumb changes without a full track change
      emit("engine://thumbs", { thumbUp: data.thumbUp, thumbDown: data.thumbDown });
    }
    updateMediaSession(data);
  }

  // ---- stations -----------------------------------------------------------
  // Full collection comes from Pandora's own web API (same-origin, session
  // cookies); the visible rail (recents) is the fallback if that fails.
  var lastStations = "";
  var fullStations = null; // [{id, name}] when the API works
  function stationEls() {
    return document.querySelectorAll('[data-qa="now_playing_station_list_station"]');
  }
  function collectStations() {
    var payload;
    if (fullStations && fullStations.length) {
      payload = {
        stations: fullStations.map(function (s) { return s.name; }),
        ids: fullStations.map(function (s) { return s.id; }),
        active: txt("station"),
      };
    } else {
      var els = stationEls();
      var names = [];
      for (var i = 0; i < els.length; i++) {
        names.push((els[i].textContent || "").trim());
      }
      payload = { stations: names, ids: null, active: txt("station") };
    }
    var s = JSON.stringify(payload);
    if (s !== lastStations) {
      lastStations = s;
      emit("engine://stations", payload);
    }
  }

  function refreshStationList() {
    var m = document.cookie.match(/csrftoken=([^;]+)/);
    if (!m || !window.fetch) return;
    var bodies = ['{"pageSize":500}', "{}"];
    var attempt = function (i) {
      if (i >= bodies.length) return;
      fetch("/api/v1/station/getStations", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-CsrfToken": m[1] },
        body: bodies[i],
        credentials: "same-origin",
      })
        .then(function (r) {
          if (!r.ok) throw new Error("http " + r.status);
          return r.json();
        })
        .then(function (j) {
          var arr = (j && (j.stations || j.items)) || [];
          if (arr.length) {
            fullStations = arr
              .map(function (s) {
                return { id: String(s.stationId || s.pandoraId || ""), name: s.name || "" };
              })
              .filter(function (s) { return s.id && s.name; });
            LOG("station list:", fullStations.length, "stations via API");
            collectStations();
          } else {
            throw new Error("empty list");
          }
        })
        .catch(function (e) {
          LOG("getStations body#" + i + " failed:", e && e.message);
          attempt(i + 1);
        });
    };
    attempt(0);
  }

  function attrPressed(name) {
    var el = qa(name);
    if (!el) return false;
    return el.getAttribute("aria-pressed") === "true" || el.classList.contains("is-active");
  }

  // Pandora renders pause_button while playing and play_button while paused —
  // the button that exists IS the state. (audio elements lie during crossfade.)
  function isPausedUi() {
    if (qa("pause")) return false;
    if (qa("play")) return true;
    return !isActuallyPlaying();
  }

  // ---- playhead (for lyric sync) ----------------------------------------
  var lastPosVal = -1;
  var lastPosMoveAt = Date.now();
  function emitPlayhead() {
    var a = audioEl();
    if (!a) return;
    var pos = a.currentTime || 0;
    if (Math.abs(pos - lastPosVal) > 0.05) {
      lastPosVal = pos;
      lastPosMoveAt = Date.now();
    }
    emit("engine://playhead", {
      position: pos,
      duration: isFinite(a.duration) ? a.duration : 0,
      paused: isPausedUi(),
      volume: a.volume,
    });
  }

  // ---- stuck-playback recovery -------------------------------------------
  // After a long pause Pandora's stream URL can expire: pressing play spins
  // forever until the page is refreshed. Detect "play requested but nothing
  // moved", then escalate: re-click play → reload the page and auto-resume.
  var recovery = { attemptAt: 0, stage: 0 };
  function armRecovery() {
    recovery.attemptAt = Date.now();
    recovery.stage = 0;
  }
  setInterval(function () {
    if (!recovery.attemptAt) return;
    if (Date.now() - lastPosMoveAt < 3000) {
      recovery.attemptAt = 0; // audio is flowing — recovered/normal
      return;
    }
    var waited = Date.now() - recovery.attemptAt;
    if (recovery.stage === 0 && waited > 12000) {
      recovery.stage = 1;
      LOG("recovery: playback never started, re-clicking play");
      var b = playPauseButton();
      if (b) b.click();
    } else if (recovery.stage === 1 && waited > 25000) {
      recovery.stage = 2;
      recovery.attemptAt = 0;
      var lastReload = 0;
      try {
        lastReload = parseInt(sessionStorage.getItem("jarlid-last-reload") || "0", 10);
      } catch (e) {}
      if (Date.now() - lastReload < 180000) {
        LOG("recovery: reload wanted but on cooldown (3min)");
        return;
      }
      LOG("recovery: reloading page to refresh the stream");
      try {
        sessionStorage.setItem("jarlid-last-reload", String(Date.now()));
        sessionStorage.setItem("jarlid-resume", "1");
      } catch (e) {}
      location.reload();
    }
  }, 2000);

  // After a recovery reload, resume playback once the player is ready.
  function maybeResumeAfterReload() {
    var resume = false;
    try {
      resume = sessionStorage.getItem("jarlid-resume") === "1";
      if (resume) sessionStorage.removeItem("jarlid-resume");
    } catch (e) {}
    if (!resume) return;
    var tries = 0;
    var timer = setInterval(function () {
      tries++;
      if (qa("pause")) {
        clearInterval(timer); // already playing
      } else if (qa("play") && txt("title")) {
        LOG("recovery: resuming playback after reload");
        clearInterval(timer);
        qa("play").click();
        armRecovery();
      } else if (tries > 30) {
        clearInterval(timer);
      }
    }, 1000);
  }

  // ---- MediaSession / Windows SMTC --------------------------------------
  function updateMediaSession(data) {
    if (!("mediaSession" in navigator)) return;
    try {
      navigator.mediaSession.metadata = new window.MediaMetadata({
        title: data.title,
        artist: data.artist,
        album: data.album,
        artwork: [
          { src: data.artFallback || data.art, sizes: "500x500", type: "image/jpeg" },
          { src: data.art, sizes: "1080x1080", type: "image/jpeg" },
        ],
      });
    } catch (e) {}
  }

  function setHandlers() {
    if (!("mediaSession" in navigator)) return;
    var set = function (action, fn) {
      try {
        navigator.mediaSession.setActionHandler(action, fn);
      } catch (e) {}
    };
    set("play", function () { cmd("play"); });
    set("pause", function () { cmd("pause"); });
    set("nexttrack", function () { cmd("skip"); });
    set("previoustrack", function () { cmd("replay"); }); // radio has no real "prev"
    set("stop", function () { cmd("pause"); });
  }

  function reflectPlaybackState() {
    if (!("mediaSession" in navigator)) return;
    navigator.mediaSession.playbackState = isPausedUi() ? "paused" : "playing";
  }

  // ---- commands (called from host via eval, and by SMTC handlers) --------
  function click(name) {
    var el = qa(name);
    if (el) el.click();
  }
  // Pandora swaps the button's data-qa between play_button (paused) and
  // pause_button (playing) — target whichever exists.
  function playPauseButton() {
    return qa("play") || qa("pause");
  }

  function cmd(name, arg) {
    LOG("cmd:", name);
    switch (name) {
      // Play/pause go through Pandora's own button: its logic always targets the
      // CURRENT track. Driving <audio> elements directly resurrected stale
      // leftover tracks after skips (play() on the wrong element).
      case "play": {
        var b1 = playPauseButton();
        if (!isActuallyPlaying() && b1) {
          b1.click();
          armRecovery();
        }
        break;
      }
      case "pause": {
        var b2 = playPauseButton();
        if (isActuallyPlaying() && b2) b2.click();
        break;
      }
      case "toggle": {
        var wasPlaying = isActuallyPlaying();
        var b3 = playPauseButton();
        if (b3) {
          b3.click();
          if (!wasPlaying) armRecovery();
        } else LOG("toggle: no play/pause button found");
        break;
      }
      case "skip": click("skip"); break;
      case "replay": click("replay"); break;
      case "thumbUp": click("up"); break;
      case "thumbDown": click("down"); break;
      default:
        // "playStation:<id>" — navigate to any station in the collection
        if (name.indexOf("playStation:") === 0) {
          location.assign("/station/play/" + name.slice(12));
        }
        // "station:<index>" — click the nth station in Pandora's station rail
        else if (name.indexOf("station:") === 0) {
          var idx = parseInt(name.slice(8), 10);
          var els = stationEls();
          if (els[idx]) els[idx].click();
          else LOG("station index out of range", idx, els.length);
        }
        break;
    }
    setTimeout(snapshot, 150);
    setTimeout(reflectPlaybackState, 150);
  }
  window.__PANDORA_BRIDGE__ = { cmd: cmd, snapshot: snapshot };

  // ---- wiring ------------------------------------------------------------
  function start() {
    setHandlers();
    // React SPA: observe the whole tree, debounce snapshots.
    var pending = null;
    var obs = new MutationObserver(function () {
      if (pending) return;
      pending = setTimeout(function () {
        pending = null;
        snapshot();
        reflectPlaybackState();
      }, 400);
    });
    // childList+subtree only (no characterData) to keep this light on Pandora's SPA.
    obs.observe(document.body, { subtree: true, childList: true });

    // audio events keep play/pause + SMTC state honest
    document.addEventListener(
      "play",
      function () { reflectPlaybackState(); snapshot(); },
      true
    );
    document.addEventListener("pause", function () { reflectPlaybackState(); }, true);

    setInterval(emitPlayhead, 500);
    setInterval(snapshot, 3000); // safety net
    setTimeout(refreshStationList, 5000); // after login/session settles
    setInterval(refreshStationList, 300000);
    maybeResumeAfterReload();
    // Heartbeat lets the host detect a dead/wedged page and auto-reload it.
    setInterval(function () {
      emit("engine://heartbeat", { t: 1 });
    }, 5000);
    // Pandora pauses long sessions behind a "Still listening?" prompt — keep going.
    setInterval(function () {
      var btns = document.querySelectorAll("button");
      for (var i = 0; i < btns.length; i++) {
        var t = (btns[i].textContent || "").trim();
        if (/still listening|keep listening|i'?m listening|continue listening/i.test(t)) {
          LOG("auto-clicking still-listening prompt:", t);
          btns[i].click();
          return;
        }
      }
    }, 4000);
    snapshot();
    emit("engine://ready", { ready: true });
    LOG("started; hasTitle:", !!txt("title"), "loginPage:", isLoginPage());
  }

  if (document.body) start();
  else window.addEventListener("DOMContentLoaded", start);
})();
