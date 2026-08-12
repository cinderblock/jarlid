# Station Galaxy — a bubble/graph view of the collection (v1.5.0 idea)

Plan path: `plans/station-galaxy.md`
Status: **not started.** Parked deliberately until the v1.3.x/v1.4.x fixes land.

## The idea (user's, 2026-08-11)

> "I would even consider adding a bubble chart view that shows visual clusters of my
> station categories, bigger bubbles for stations that I listen to more, links for
> artists/themes/genres that are shared between stations."

A second way to look at the collection: not a list, but a map. Bubble per station, size by
how much it actually gets played, edges where two stations share seed artists or genres,
and clusters falling out of that naturally.

## Why this does not contradict "I hate 2D lists"

The same user, in the same message, asked for the Stations *list* to stop being a grid —
"don't make my eyes scan in two dimensions at once". That is not in tension with this.
Scanning a grid to **find a known name** is a search task, and forcing it into two
dimensions makes it slower. A galaxy view is for **noticing structure you did not know was
there** — which needs two dimensions, because the whole content is the relationships.

So: the list stays one column, and this is a *separate view*, toggled — never a replacement.

## The key insight: this is downstream of the export walk

The graph's data is exactly what the station-preferences export already collects. One
`station.getStation` per station returns `music.artists[]`, `music.songs[]` and `genre[]`,
which is precisely what the edges are made of. See `plans/station-prefs-export.md` for the
verified field shapes.

Consequences:

- **No new API surface is needed.** Building the graph is the export walk with a different
  consumer.
- **It inherits the pacing rule.** That walk is deliberately serial with a gap between
  stations because an export runs while music is playing and must not look like a scrape.
  A visualisation that silently hammers the account to redraw itself would be far worse
  than no visualisation. Build it on a **cached** snapshot, refreshed on demand.
- Practically: persist a graph snapshot in the cache dir, offer "refresh" explicitly, and
  show when the data was gathered.

## ⚠️ Open question: where does "listen to more" come from?

**Unverified, and it is the load-bearing unknown for bubble size.** Nothing observed so far
exposes a per-station play count:

- `user.getStationList` returns name/token/id and the special-station flags. No counts.
- `station.getStation` (extended) returns seeds, feedback and settings. No counts.

Two candidate answers, in preference order:

1. **Count it ourselves.** Jarlid knows every track it plays and which station it came from
   — it already persists `last-station.json` and a recently-played list in `localStorage`.
   A per-station tally is cheap, accurate for *this* client, and needs no API at all. The
   honest caveat is that it only counts listening done through Jarlid, so a fresh install
   starts flat and the bubbles grow over time.
2. **Ask Pandora.** Check whether any endpoint carries a play count or a last-played
   timestamp (`getStationList` has been seen with `dateCreated`; a `lastPlayed` field may
   exist on the REST station list, which is richer than the tuner one). Worth one look with
   `dump-shapes`/`dump-station-shape` before assuming it does not exist.

Falling back on "number of thumbs" as a proxy for listening would be wrong and should be
resisted — it measures opinion, not time.

## Sketch, if it gets built

- Canvas, not SVG: a few hundred nodes with a force simulation is fine on canvas and
  miserable in DOM nodes.
- A small force-directed layout written directly rather than pulling in d3 — the repo keeps
  its dependency list short, and this needs repulsion + spring + centring, not a library.
- Edge weight = shared seed artists (strong) and shared genres (weak). Probably hide edges
  below a threshold or it becomes a hairball.
- Colour by the same three groups the list now uses (mixes / yours / genre), so the two
  views agree about what a station *is*.
- Click a bubble to play that station, matching the list's behaviour.

## Things not to do

- Don't refresh the graph by walking the account on open. Cache, and refresh on request.
- Don't replace the list view with it.
- Don't use thumb counts as a stand-in for play counts.
