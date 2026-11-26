# BookWalker provider for BookOrbit

Metadata from [BookWalker](https://bookwalker.com), the English store for licensed manga, light
novels and webtoons, for [BookOrbit](https://github.com/bookorbit/bookorbit). Every record is one
volume or chapter with its own ISBN, release date, cover and description, plus the series' genres
and tags. File names are read the same way as the
[MangaBaka fork](../mangabaka) reads them, and records are titled the same way:
`Blue Exorcist - Vol. 5`, `Booty Royale: Never Go Down Without a Fight! - Vol. 9-10`,
`Goblin Slayer Side Story: Year One - Ch. 51`, with the chapter (else the volume) as the series
index.

This is bookwalker.com, the English store run by M12 Media since March 2026. The old
`global.bookwalker.jp` store and the Japanese `bookwalker.jp` are not read.

## Use it

In your BookOrbit `docker-compose.yml`:

```yaml
services:
  bookwalker:
    build:
      context: https://github.com/KeePach/bookorbit-mangabaka-modification.git#main:providers/bookwalker
      dockerfile: Dockerfile
    image: bookwalker-provider:local
    restart: unless-stopped
    ports:
      - "127.0.0.1:8082:8080"
    environment:
      PUBLIC_URL: http://localhost:8082
    volumes:
      - bookwalker-data:/data

volumes:
  bookwalker-data:
```

(If the file already has a top-level `volumes:` section, add `bookwalker-data:` to it instead.)

Then `docker compose up -d --build bookwalker`, and add the provider in BookOrbit under
**Settings > Metadata > Providers > External providers** with the URL `http://bookwalker:8080`.

## How it finds books

BookWalker's own search refuses requests that do not come from a browser, so the provider never
uses it. Instead it keeps the list of every series, volume and chapter that BookWalker publishes in
its sitemap (about 117,000 entries, 14 small files) and matches titles against it locally. A
search then reads one series page and one volume or chapter page.

**When the list is downloaded:** the first time you search after the provider starts with no
saved list, and after that only on the first search of a later day. That search is answered from
the saved list straight away and the new list replaces it a few seconds later. A day without
searches downloads nothing, and a restart reads the saved list from `/data` instead of downloading
it. (A day is a UTC day.) To fetch a fresh list right now anyway, which is never needed in normal
use: `docker exec bookorbit-app wget -qO- --post-data= http://bookwalker:8080/catalog/refresh`.

**Matching:**

- When BookOrbit sends the series (from the folder), that name is searched and the file name only
  says which volume or chapter it is (`c051`, `v04`). Otherwise the series is read from the file
  name, cut at its first volume/chapter marker: `Blue Exorcist v04 c05`, `Series - Vol. 9-10`,
  `Series_v04_c05`, `Series.v04.c05`. Brackets (`(2012)`, `[Digital]`) and an edition word (`the
  Comic`, `(Light Novel)`) are left out.
- Names are compared as BookWalker writes them into its links (`Re:ZERO` is `rezero`), and a series
  has to share more than half of the name's words, so a title BookWalker does not sell answers no
  matches instead of a wrong one.
- A volume is picked as the series page lists it (`Volume 5`, `Vols. 9-10`). A range asks for the
  omnibus with that range, else the volume holding its first number. Words after the series' name
  pick the part: `Ascendance of a Bookworm Part 2 Vol 1`.
- A chapter is picked from the sitemap and checked against the chapter page's own title.
- When BookWalker does not sell the asked-for volume or chapter on its own (most manga are sold as
  volumes only), the volume holding it is used if the file name says the volume
  (`Blue Exorcist v05 c38` → volume 5, titled `Blue Exorcist - Vol. 5 Ch. 38`), else the series
  itself, still titled with the chapter (`Blue Exorcist - Ch. 5`).
- A manga and its novel often share a name. Both come back, each labelled with its own media type
  (manga and webtoons `comic`, novels `book`), and BookOrbit picks by the book's file: a CBZ or CBR
  prefers the manga, an EPUB the novel.

## Mapping

- **Title** `<series> - <volume/chapter>` as above; **series** the series' name with the chapter
  (else the volume, else nothing) as its index; a range sits at its first number.
- **Ids** `v-<id>` for a volume, `c-<id>` for a chapter, `s-<id>` for a series, followed by the
  volume/chapter the record was titled with (`v-1K4M8FFM0DZG_vol9-10`, `c-3EW93XA7Z120_v51`), so a
  lookup by id writes the same title again.
- **Authors** the story credits, then the art (`AUTHOR`, `ARTIST`); translators, letterers and
  editors are left out.
- **Publisher**, **ISBN-13**, **release date**, **language**, **description** and **cover**
  (1200 px) from the volume or chapter page; **genres** and **tags** from the series page. The
  description is the full text behind "Read more", not the tagline some volumes show first.
- **Mature titles** show a placeholder cover to visitors who are not signed in, so their records
  have no cover and BookOrbit keeps the one it has.
- Audiobooks are skipped. There is no ISBN search (it would need BookWalker's own search).

## Settings

| Variable | Default | |
|---|---|---|
| `PORT` | `8080` | |
| `AUTH_TOKEN` | unset | When set, every route but `/provider/info` requires this exact `Authorization` header |
| `PUBLIC_URL` | unset | Where a browser reaches this provider (`http://localhost:8082` with the port above). When set, BookOrbit shows each BookWalker id as a link that opens the volume, chapter or series on bookwalker.com |
| `DATA_DIR` | `/data` | Where the list is saved; empty keeps it in memory only |
| `REQUEST_INTERVAL_MS` | `1000` | Gap between pages read from bookwalker.com |
| `PAGE_CACHE_MINUTES` | `60` | How long a read series, volume or chapter page is kept |
| `USER_AGENT` | `bookorbit-bookwalker-provider/<version>` | |
| `BOOKWALKER_URL` / `BOOKWALKER_STATIC_URL` | `https://bookwalker.com` / `https://static.bookwalker.com` | For tests |

BookWalker publishes no rate limit; one page a second is far less than its own site loads while
someone browses. A caller that would wait more than 15 seconds for its turn gets `429` with
`Retry-After` instead.

## Endpoints

| Route | |
|---|---|
| `GET /provider/info` | Key `bookwalker`, media `book` and `comic`, lookup, `series`/`seriesIndex` search parameters |
| `GET /search?query=&author=&mediaType=&series=&seriesIndex=&limit=` | `limit` defaults to 5, at most 20 |
| `GET /book/{id}` | An id from a search |
| `GET /open/{id}` | Redirects a browser to the id's page on bookwalker.com (no token needed) |
| `POST /catalog/refresh` | Fetch a fresh list now (answers `202` at once) |

Status codes: `400` no query, `401` bad token, `404` unknown id, `429` with `Retry-After` when
bookwalker.com throttles or the queue is long, `502` when it fails or refuses the provider.

## What it reads, and what could break it

Only what bookwalker.com publishes on purpose: its sitemap, the schema.org `Book` record and `og:`
tags of its pages, link targets (`/series/…`, `/browse?genres.include…`) and the labels of the
details list (`PUBLISHER`, `AUTHOR`). Never CSS class names, which change with every deploy, and
never `/api/`, which its `robots.txt` asks robots to leave alone. It names itself in its
User-Agent rather than pretending to be a browser. If BookWalker starts refusing it (`502` with
`bookwalker.com answered 403` in the log), nothing in the provider can fix that.

## Working on it

- Bump `VERSION` in `src/main.rs` whenever you change behavior: BookOrbit caches answers per
  provider version. Use BookOrbit's **Search fresh** after a change.
- The fixtures in `testdata/` are real bookwalker.com pages captured on 2026-11-26, cut down to
  the parts the provider reads, and a slice of the real sitemap.
- `src/label.rs` is copied from the MangaBaka provider so both read file names alike; keep them in
  step.

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```
