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
  LOG("v3 injected @", location.href, "top-frame:", window.top === window);
  // Only run in the top frame — Pandora has cross-origin iframes we must ignore.
  if (window.top !== window) {
    LOG("skipping sub-frame");
    return;
  }

  var QA = {
    title: "playing_track_title",
    artist: "playing_artist_name",
    album: "playing_album_name",
    station: "station_active_name",
    art: "album_active_image",
    artMini: "mini_track_image",
    play: "play_button",
    skip: "skip_button",
    replay: "replay_button",
    up: "thumbs_up_button",
    down: "thumbs_down_button",
    volume: "volume_slider",
  };

  function qa(name) {
    return document.querySelector('[data-qa="' + QA[name] + '"]');
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
  var everHadTitle = false;
  function snapshot() {
    var title = txt("title");
    if (!title) {
      // No player yet: either still loading, or we genuinely need a login.
      // Never signal login once we've seen a real track (transient re-renders).
      if (everHadTitle) return;
      var loginish =
        isLoginPage() || /(^\/$)|login|signin|sign-in/i.test(location.pathname);
      if (loginish) emit("engine://needs-login", { needsLogin: true });
      return;
    }
    everHadTitle = true;
    // Title present => the player is up. Never show the login card.
    var data = {
      title: title,
      artist: txt("artist"),
      album: txt("album"),
      station: txt("station"),
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
  var lastStations = "";
  function stationEls() {
    return document.querySelectorAll('[data-qa="now_playing_station_list_station"]');
  }
  function collectStations() {
    var els = stationEls();
    var names = [];
    for (var i = 0; i < els.length; i++) {
      names.push((els[i].textContent || "").trim());
    }
    var payload = { stations: names, active: txt("station") };
    var s = JSON.stringify(payload);
    if (s !== lastStations) {
      lastStations = s;
      emit("engine://stations", payload);
    }
  }

  function attrPressed(name) {
    var el = qa(name);
    if (!el) return false;
    return el.getAttribute("aria-pressed") === "true" || el.classList.contains("is-active");
  }

  // Pandora's own play button is the most reliable playing/paused signal: its
  // aria-label reads "Pause" while playing and "Play" while paused. The <audio>
  // elements lie during crossfade/preload.
  function isPausedUi() {
    var el = qa("play");
    var aria = el ? el.getAttribute("aria-label") || "" : "";
    if (/pause/i.test(aria)) return false;
    if (/play/i.test(aria)) return true;
    var a = audioEl();
    return a ? a.paused : true;
  }

  // ---- playhead (for lyric sync) ----------------------------------------
  function emitPlayhead() {
    var a = audioEl();
    if (!a) return;
    emit("engine://playhead", {
      position: a.currentTime || 0,
      duration: isFinite(a.duration) ? a.duration : 0,
      paused: isPausedUi(),
      volume: a.volume,
    });
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
  function cmd(name, arg) {
    LOG("cmd:", name);
    switch (name) {
      // Play/pause go through Pandora's own button: its logic always targets the
      // CURRENT track. Driving <audio> elements directly resurrected stale
      // leftover tracks after skips (play() on the wrong element).
      case "play":
        if (!isActuallyPlaying()) click("play");
        break;
      case "pause":
        if (isActuallyPlaying()) click("play");
        break;
      case "toggle":
        click("play");
        break;
      case "skip": click("skip"); break;
      case "replay": click("replay"); break;
      case "thumbUp": click("up"); break;
      case "thumbDown": click("down"); break;
      default:
        // "station:<index>" — click the nth station in Pandora's station rail
        if (name.indexOf("station:") === 0) {
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
