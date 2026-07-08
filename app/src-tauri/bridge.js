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
  LOG("injected @", location.href, "top-frame:", window.top === window);
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
  function txt(name) {
    var el = qa(name);
    if (!el) return "";
    // Pandora wraps text in a scrolling marquee that CLONES its content element
    // for long strings (producing doubled text). Read only the first clone.
    var content = el.querySelector(".Marquee__wrapper__content");
    return ((content || el).textContent || "").trim();
  }
  // Pandora keeps multiple <audio> elements (it preloads the next track), so the
  // first one is often the previous/ended track. Pick the live one.
  function audioEl() {
    var els = document.querySelectorAll("audio");
    if (!els.length) return null;
    // 1) the element actually playing right now
    for (var i = 0; i < els.length; i++) {
      if (!els[i].paused && !els[i].ended) return els[i];
    }
    // 2) a loaded, not-ended element with a real duration (covers paused state)
    for (var j = 0; j < els.length; j++) {
      if (!els[j].ended && isFinite(els[j].duration) && els[j].duration > 0)
        return els[j];
    }
    // 3) fall back to the most recently added element
    return els[els.length - 1];
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

  function attrPressed(name) {
    var el = qa(name);
    if (!el) return false;
    return el.getAttribute("aria-pressed") === "true" || el.classList.contains("is-active");
  }

  // ---- playhead (for lyric sync) ----------------------------------------
  function emitPlayhead() {
    var a = audioEl();
    if (!a) return;
    emit("engine://playhead", {
      position: a.currentTime || 0,
      duration: isFinite(a.duration) ? a.duration : 0,
      paused: a.paused,
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
    var a = audioEl();
    if (a) navigator.mediaSession.playbackState = a.paused ? "paused" : "playing";
  }

  // ---- commands (called from host via eval, and by SMTC handlers) --------
  function click(name) {
    var el = qa(name);
    if (el) el.click();
  }
  function cmd(name, arg) {
    switch (name) {
      case "play":
      case "pause":
      case "toggle": {
        var a = audioEl();
        // play_button toggles play/pause; only click when we need to change state
        if (name === "toggle") click("play");
        else if (a && a.paused && name === "play") click("play");
        else if (a && !a.paused && name === "pause") click("play");
        break;
      }
      case "skip": click("skip"); break;
      case "replay": click("replay"); break;
      case "thumbUp": click("up"); break;
      case "thumbDown": click("down"); break;
      default: break;
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
    snapshot();
    emit("engine://ready", { ready: true });
    LOG("started; hasTitle:", !!txt("title"), "loginPage:", isLoginPage());
  }

  if (document.body) start();
  else window.addEventListener("DOMContentLoaded", start);
})();
