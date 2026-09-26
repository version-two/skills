# bazos-cli

A command-line tool that searches bazos.sk and bazos.cz and returns the parsed ads. JSON output is the default, so agents can use it directly.

Version Two s.r.o. – maintainer Mario Chamuty.

## Build

Prebuilt binaries: [releases](https://github.com/version-two/skills/releases) tagged `bazos-v*`.

```sh
cargo build --release -p bazos-cli   # from tools/, binary: target/release/bazos(.exe)
cargo test -p bazos-cli
```

## Commands

### `bazos search [QUERY]`

| Flag | Site parameter | Notes |
|---|---|---|
| `QUERY` | `hledat` | search text, optional |
| `-s, --site sk\|cz` | domain | default `sk` |
| `-c, --category SLUG` | subdomain | e.g. `mobil`, `auto`, `reality` (see `bazos categories`) |
| `--subcategory SLUG` | path | e.g. `apple`, `prodam/byt`; checked against the live list, needs `--category` |
| `-l, --location TEXT` | `hlokalita` | postal code (`81101`) or place name (`Žilina`) |
| `-r, --radius KM` | `humkreis` | needs `--location` |
| `--price-min N` / `--price-max N` | `cenaod` / `cenado` | in the site currency (EUR / CZK) |
| `-o, --sort` | `order` | `newest` (default), `price-asc`, `price-desc`, `most-viewed`, `least-viewed` |
| `-n, --limit N` | – | default 20; fetches as many 20-item pages as needed |
| `--offset N` | `crz` / path | skips N listings, counted in site order |
| `--no-top` | – | drops paid TOP listings; bazos lists all of them first (about 400 in mobil/apple) even when they do not match the search, and the tool pages past them |
| `--details` | – | also fetches each ad's detail page (adds `details: [...]`) |
| `--concurrency N` | – | parallel detail fetches, default 4 |

### `bazos ad URL...`

Parses one or more ad detail pages. Pass the ad URL exactly as a search returned it: bazos serves the ad only on its category subdomain with the correct slug. Returns the full description, all image URLs, seller name, postal code, place, latitude and longitude, views, price, date, and the category and subcategory breadcrumb.

### `bazos categories [CATEGORY] [-s sk|cz]`

Without an argument, lists the top-level category slugs. With a category, lists its subcategories. Each entry has `category`, `slug`, `name` and `url`. A menu entry that links into another category keeps that other category in `category`.

### Global flags

`-f, --format json|jsonl|table`, `--delay-ms` (minimum gap between requests, default 300), `--timeout` (seconds, default 30).

`jsonl` prints one object per line: search listings (or ads when `--details` is set), ads, or categories.

## Output

Prices: `{raw, kind, amount, currency}`. `kind` is one of `amount`, `negotiable` (Dohodou), `offer` (Ponúknite), `free` (Zadarmo/Zdarma), `in_text` (V texte/V textu) or `other`, which keeps the unrecognised text in `raw`. Dates are ISO `YYYY-MM-DD`.

`total: 0` with an empty `listings` array means bazos reported no matches. Every other failure is an error.

## Errors

Errors go to stderr as `{"error": code, "message": ...}` with a non-zero exit code:

| Exit | Code | Meaning |
|---|---|---|
| 2 | `invalid_query`, `invalid_ad_url` | bad arguments, unknown subcategory |
| 3 | `ad_gone` | ad removed (HTTP 404/410) |
| 4 | `http_error`, `transport_error` | network failure or non-2xx response, after 3 attempts for 429/5xx/timeouts |
| 5 | `parse_error` | page structure changed; nothing partial is returned |

## Examples

```sh
bazos search "iphone 13" -c mobil --subcategory apple -l 81101 -r 50 --price-min 100 --price-max 500 -o price-asc -n 40
bazos search -s cz -c reality --subcategory prodam/byt -l Brno -r 10 --no-top --details -f jsonl
bazos ad https://mobil.bazos.sk/inzerat/195868345/predam-iphone-14-pro-256-gb.php
bazos categories auto
```
