---
name: bazos
description: Search and read classified ads on bazos.sk and bazos.cz (Slovak/Czech second-hand marketplace) via the bundled `bazos` CLI. Use when the user wants to find, compare, monitor or extract bazos listings – by text, category, subcategory, location + radius, price range, sort order – or read full ad details (description, images, seller, coordinates). Triggers: bazos, bazoš, bazar, inzerát, second-hand offers in SK/CZ.
---

# bazos

Call the bundled launcher with its full path (it downloads the matching release binary for Linux, macOS or Windows on first run):

```sh
B="${CLAUDE_PLUGIN_ROOT}/scripts/bazos"
```

Output is JSON on stdout. Errors are JSON on stderr (`{"error": code, "message": ...}`) with a non-zero exit code. Do not scrape bazos with curl; use this tool.

## Workflow

1. Unknown category? Run `$B categories -s sk` (top level) and `$B categories mobil -s sk` (subcategories). Slugs differ between SK and CZ (`dom` vs `dum`, `motocykle` vs `motorky`, `oblecenie` vs `obleceni`), so list the right site.
2. Search: `$B search "text" [filters]`. Results come back as `listings[]`, and `total` is the site's full match count.
3. Read ads: `$B ad URL...` using URLs exactly as the search returned them, or add `--details` to the search.

## search flags

| Flag | Meaning |
|---|---|
| `QUERY` (positional) | search text, optional |
| `-s sk\|cz` | site, default `sk`; prices are EUR (sk) or CZK (cz) |
| `-c SLUG` | category, e.g. `mobil`, `auto`, `reality`, `elektro`, `pc` |
| `--subcategory SLUG` | e.g. `apple`, `prodam/byt`, or a parent such as `prodam`; checked live, needs `-c` |
| `-l TEXT` | postal code (`81101`) or place (`Žilina`, `Brno`) |
| `-r KM` | radius around `-l`, needs `-l` |
| `--price-min N` / `--price-max N` | price range |
| `-o` | `newest` (default), `price-asc`, `price-desc`, `most-viewed`, `least-viewed` |
| `-n N` / `--offset N` | how many to return (default 20, pages fetched automatically) and how many to skip |
| `--no-top` | drop paid TOP listings. **Use it by default**: bazos lists every TOP ad first (hundreds in busy categories), and they often do not match the query. The tool pages past them, so expect a few seconds of extra fetching |
| `--details` | fetch every ad's detail page into `details[]` (slower: one request per ad) |
| `--concurrency N` | parallel detail fetches, default 4 |

Global: `-f json|jsonl|table`, `--delay-ms` (default 300), `--timeout` (s, default 30).

## Data

Listing: `id, url, category, title, snippet, price, location, postal_code, views, date (YYYY-MM-DD), top, thumbnail`.

Ad (`ad` / `--details`): `id, url, site, title, description (full, with newlines), price, date, top, seller_name, location, postal_code, latitude, longitude, views, category{name,url}, subcategory{name,url}, images[]`.

Price: `{raw, kind, amount, currency}`. `kind` is `amount` | `negotiable` | `offer` | `free` | `in_text` | `other`. Only `amount` has a number. For `in_text`, read the price from the description.

## Rules

- `total: 0` with empty listings is a real "no matches". Any error is a failure; report it, never present it as "nothing found".
- Exit codes: 2 invalid query or unknown subcategory (the message lists the valid slugs or the correct category, so fix and retry), 3 ad removed, 4 network/HTTP (retry later), 5 page structure changed (the tool needs updating; tell the user), 6 the launcher could not download or verify the binary (tell the user).
- Results follow bazos' own matching, which is loose. Filter `listings` yourself for strict criteria such as model or condition.
- Keep `-n` moderate (≤ 100) and use `--details` only for the ads you actually need. Every page is a live request to bazos.
- Seller phone numbers are not extracted.

## Examples

```sh
$B search "iphone 13" -c mobil --subcategory apple -l 81101 -r 50 --price-min 100 --price-max 500 -o price-asc --no-top -n 40
$B search -s cz -c reality --subcategory prodam/byt -l Brno -r 10 --no-top --details -f jsonl
$B search "horský bicykel" -c sport -o newest --no-top -f table
$B ad https://mobil.bazos.sk/inzerat/195868345/predam-iphone-14-pro-256-gb.php
```

Source: https://github.com/version-two/skills/tree/main/tools/bazos (Rust).
