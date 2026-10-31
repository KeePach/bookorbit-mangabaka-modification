# MangaBaka provider for BookOrbit (modified fork)

A fork of the official MangaBaka metadata provider for [BookOrbit](https://github.com/bookorbit/bookorbit),
tuned for file names that carry chapter numbers, chapter names, or a romanized (non-English) title.
Titles and series names are always returned in English.

## What's changed

- **Title cut at the chapter/volume.** Everything after the first `Vol.`/`Ch.` marker (chapter name,
  author, language, scanlation group) is dropped from the search query, so
  `Series - Ch.0006 - Chapter Name - Author (en) [group]` searches for `Series`.
- **Series index = chapter, else volume, else nothing.** `Vol. 2 Ch. 8` gives `8`, `Vol. 2` gives `2`,
  neither gives an empty index. The full `Vol. 2 Ch. 8` label stays in the title only.
- **Number cleanup.** Leading zeros go (`Ch.0006` becomes `6`), and so do meaningless trailing decimal
  zeros (`12.50` becomes `12.5`).
- **Wider marker detection.** `(`, `[` and `_` around the number are accepted (`Series (Vol 2)`).
- **Shorter retries.** A search that finds nothing is retried: without a trailing edition word
  (`the Comic`, `Manga`, `(Light Novel)`), then with the first half of the title.
- **Better ordering.** Results are ordered by author match, exact name, then most shared words.
- **`displayTitle`.** When another name of the series (romanized or native) fits the query better than the
  English title, it is sent as `displayTitle`. `title` and the series stay English. This needs a BookOrbit
  build that reads `displayTitle` (see the BookOrbit fork below); other builds ignore it.
- **Title language. The returned title and series name are always English, whatever language the search used (a Japanese search does not return a Japanese title). With no English name, it falls back to MangaBaka’s main title, then the romanized title.

## Use it

In your BookOrbit `docker-compose.yml`:

```yaml
  mangabaka:
    build:
      context: https://github.com/KeePach/bookorbit-mangabaka-modification.git#main:providers/mangabaka
      dockerfile: Dockerfile
    image: mangabaka-provider:local
    restart: unless-stopped
```

Then `docker compose up -d --build mangabaka`, and add the provider in BookOrbit with the URL
`http://mangabaka:8080`.

## Working on it

- Bump `VERSION` in `src/main.rs` whenever you change behavior. BookOrbit caches answers per provider
  version, so an old empty answer keeps being served until the version changes.
- Keep `Cargo.toml` and `Cargo.lock` at the same package version; the Dockerfile builds with `--locked`.
- Use BookOrbit's **Search fresh** after a change, to skip its cache.

Companion fork: [KeePach/bookorbit](https://github.com/KeePach/bookorbit). (only modified how it compare the title so it does not drop the match)


| Variable | Default | |
|---|---|---|
| `PORT` | `8080` | |
| `AUTH_TOKEN` | unset | When set, `/search` and `/book/{id}` require this exact `Authorization` header |
| `MANGABAKA_API` | `https://api.mangabaka.org` | |
| `SEARCH_INTERVAL_MS` | `2000` | Gap between searches sent to MangaBaka |
| `LOOKUP_INTERVAL_MS` | `340` | Gap between other requests sent to MangaBaka |

MangaBaka needs no API key. It allows 30 searches and 180 other requests a minute per IP; the
default gaps stay inside both. A caller that would wait more than 15 seconds for its turn gets
`429` with `Retry-After` instead.

## Endpoints

| Route | |
|---|---|
| `GET /provider/info` | Key `mangabaka`, media `book` and `comic`, lookup, a 2-second `rateLimit`, a URL template for record links, portrait covers |
| `GET /search?query=&author=&mediaType=&limit=` | `limit` defaults to 5, at most 20 |
| `GET /book/{id}` | A MangaBaka series id, or one volume of it (`1692v3`). A series merged into another answers with the one it now lives in |

Status codes: `400` no query, `401` bad token, `404` unknown id, `429` with `Retry-After` when
MangaBaka throttles or the queue is long, `502` when MangaBaka fails.

## Mapping

- **Medium:** a record's `mediaType` is `comic`, or `book` for a series MangaBaka files as a novel,
  so a client can tell a light novel from its manga.
- **Ids** are MangaBaka's series id, with `v` and the volume for a volume (`1692v3`), so looking
  one up again answers that volume. Records carry the series' page as `url`.
- **Title** is the series' official English name, written as a shelf would ("ONE PIECE" is "One
  Piece", "SPY×FAMILY" is "Spy x Family"), with the volume when one was asked for: "Berserk, Vol. 3".
  The series name is the same without the volume.
- **Matching:** the asked-for author's series first, then a series named exactly as asked, then
  MangaBaka's order. MangaBaka's search is fuzzy, so a series none of whose names shares a word with
  the query ("June" for "Dune") is left out.
- **Authors** are MangaBaka's story credits, falling back to the artists, given name first
  ("MIURA Kentaro" is "Kentaro Miura"). An `author` in the query matches names in any order and
  ignoring separators ("Chugong" for "Chu-Gong").
- **Publisher** is the English-language publisher when there is one, else the original, and the
  **language** goes with it: `en` with an English publisher, else the original language.
- **Published** is the series' start, so it is given for the series and for volume 1 only, never
  as a later volume's date.
- **Rating** is MangaBaka's 100-point score on a 5-point scale. There is no vote count.
- **Description** is plain text; MangaBaka's Markdown is stripped.
- **Genres** read as words ("school_life" is "School Life"); MangaBaka's tags go to `tags`.
- **Cover** is the full-size original, with its width and height.
- Only active series are offered.

Known gaps: no ISBN, page count or per-volume data, since MangaBaka has none, so a later volume
has no publication date.

## Test

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

Or in a container, with no local toolchain:

```bash
docker run --rm -v "$PWD":/src -w /src rust:1.90-alpine sh -c 'apk add --no-cache musl-dev && cargo test'
```

The fixtures in `testdata/` are real MangaBaka answers captured on 2026-10-22, cut down to the
fields the provider reads. The merged record is the real 1692 with its state changed.

## Why Rust

This provider was first written in Go as well, with identical answers, and both were measured
under the same load: a stand-in for MangaBaka serving its real 171 KB search answers, 4,000 mixed
searches and lookups at 1, 8 and 32 concurrent clients, three rounds each, on linux/arm64 with 11
vCPUs (2026-10-22). Medians:

| | Go | Rust |
|---|---:|---:|
| Idle | 5.4 MiB | 2.3 MiB |
| Steady use | 12 MiB | 4 MiB |
| 32 clients at once | 21.5 MiB | 7.8 MiB |
| Peak | 57 MiB | 12 MiB |
| Requests a second at 32 clients | 1,485 | 3,065 |
| CPU for the whole run | 19.5 s | 10.5 s |
| Image | 6.2 MB | 3.8 MB (2.8 MB now) |

Go's `GOMEMLIMIT=10MiB` brought its settled memory to 14 MiB but not its peak, and cost speed. In
real use MangaBaka's 30 searches a minute keep either build near its steady figure, so Go would
have done the job too; Rust does it in less.

The Rust build was later switched to optimise for size (`opt-level = "z"`, regex without its
speed-up engines), which took the image to 2.8 MB; the figures above are from the earlier build.
