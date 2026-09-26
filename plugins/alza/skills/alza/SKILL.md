---
name: alza
description: Search and read products on Alza (alza.sk, alza.cz, alza.hu, alza.at, alza.de – the Czech/Slovak electronics and general e-shop) via the bundled `alza` CLI, which uses the official Alza app API. Use when the user wants to find, compare, filter or monitor Alza products – by text, category, producer, technical parameters (RAM, display, CPU, …), price range, stock, condition (new / open-box / used), discounts – or read full product details (parameters, images, review stats) and customer reviews. Triggers: alza, alza.sk, alza.cz, Alza e-shop, "na Alze", product prices in SK/CZ/HU/AT/DE electronics shops.
---

# alza

Call the bundled launcher with its full path (it downloads the matching release binary for Linux, macOS or Windows on first run):

```sh
A="${CLAUDE_PLUGIN_ROOT}/scripts/alza"
```

Output is JSON on stdout. Errors are JSON on stderr (`{"error": code, "message": ...}`) with a non-zero exit code. Do not scrape alza with curl; Cloudflare blocks non-app clients. Use this tool.

## Workflow

1. Free-text need ("cheap OLED laptop"): `$A search "notebook oled" -o price-asc`. The result's `related_categories[]` carry `ref`s for step 2.
2. Precise filtering (RAM, panel type, brand, …) works best on a category:
   - find it: `$A categories` → `$A categories <ref>` (entries with `section: true` group subcategories and cannot be listed; drill further),
   - see its filters: `$A filters -c <ref> -f table`,
   - list it: `$A search -c <ref> --producer Lenovo --param "Typ panela=OLED" --param "Veľkosť operačnej pamäte RAM=16 GB.."`.
3. Details: `$A product <id|url>...` or add `--details` to the search. Reviews: `$A reviews <id|url> -n 50`.

A text search and `--category` cannot be combined (the Alza API has no category-scoped search). `alza filters "text"` gives the facets of a text search, and `--producer` / `--param` work on text searches too.

## search flags

| Flag | Meaning |
|---|---|
| `QUERY` (positional) | search text |
| `-c REF` | list a category instead: `ID`, `ID:TYPE:TYPE_ID` (copy `ref` exactly, e.g. `1:ACTION:17` for a promotion) or a category URL ending in `/<id>.htm` |
| `-s cz\|sk\|hu\|at\|de` | site, default `sk`; prices in CZK / EUR / HUF / EUR / EUR |
| `--lang cs\|sk\|en\|de\|hu` | response language (names, availability texts, filter labels); default the site's language |
| `--price-min N` / `--price-max N` | price range |
| `-o` | `recommended` (default), `bestselling` (categories only), `price-asc`, `price-desc`, `rating`, `newest` |
| `--stock any\|anywhere\|alza` | `anywhere` = in stock at Alza or a partner, `alza` = in stock at Alza |
| `--branch ID` | only stock in that showroom (needs `--stock alza`); ids in `filters` output |
| `--condition new\|open-box\|used` | repeatable; `open-box` = unsealed/tested, full warranty; `used` = like new or used |
| `--discounted` / `--alza-plus` / `--rated-4-plus` | promotions only / AlzaPlus+ only / 4★ and more |
| `--producer NAME\|ID` | repeatable |
| `--param "KEY=VALUE"` | checkbox filter; repeat for several values of the same filter |
| `--param "KEY=FROM..TO"` | slider filter; either side optional (`16 GB..`, `..2 kg`) |
| `-n N` / `--offset N` | how many (default 25, pages of 25 fetched automatically) and how many to skip |
| `--details` | fetch each listing's full product detail into `details[]` (2 requests per product) |

`KEY` is the filter id or its exact name, values are ids/numbers or exact labels, all from `filters` output in the same `--lang`. Slider raw numbers use the filter's unit (RAM 16 GB = `16384`), so prefer labels. Unknown producers/filters/values fail with the valid list in the message: fix and retry.

Global: `-f json|jsonl|table`, `--delay-ms` (default 300), `--timeout` (s, default 30).

## Data

Listing: `id, code, name, url, spec (one-line summary), image, price, price_note, availability (text), can_buy, rating, rating_count, promo`.

Product (`product` / `--details`): listing fields plus `availability_note, in_stock, warranty, producer_id, part_number, category, breadcrumbs[], reviews{average, rating_count, review_count, recommendation_rate, distribution[], complaint_rate, complaint_label, purchases}, images[], parameters[{name, parameters[{name, values[]}]}], documents[], description_url`. `reviews.average` is null for an unrated product; `complaint_rate` is null until 30 customers bought it.

Price: `{amount, currency, without_vat, original, raw}`. `original` is the pre-discount price or null. `price` itself is null when Alza shows no price (sale ended or price not set, `can_buy: false`; Alza's text, if any, is in `price_note`); such products cannot be bought, so leave them out of price comparisons.

Review: `rating, author, reviewed_at (ISO 8601 UTC), variant, text, positives[], negatives[], verified_purchase, translated, likes, images[]`.

## Rules

- `total: 0` with empty listings is a real "no matches". Any error is a failure; report it, never present it as "nothing found".
- Exit codes: 2 invalid query (the message says how to fix it), 3 product does not exist, 4 network/HTTP/API error (retry later; 403 means Cloudflare refused), 5 response shape changed (the tool needs updating; tell the user), 6 the launcher could not download or verify the binary (tell the user).
- Results follow Alza's own matching and relevance. Check `name`/`spec`/`parameters` yourself before claiming a product matches strict criteria.
- `price-asc` / `price-desc` use Alza's ordering, which can deviate slightly from `price.amount`; re-sort yourself when exact order matters.
- Keep `-n` moderate (≤ 100) and use `--details` only for products you actually need; every page is a live request.
- The long marketing description is not extracted; `description_url` points to it. Use `spec` and `parameters` for facts.

## Examples

```sh
$A search "thinkpad" -o price-asc -n 10 -f table
$A search -c 18842920 --producer Lenovo --param "Veľkosť operačnej pamäte RAM=32 GB.." --price-max 2500 --stock anywhere
$A search -c 18842920 --condition open-box --condition used -o price-asc
$A search iphone -s cz --lang en --discounted -f jsonl
$A categories 18890188 -f table
$A filters -c 18842920 -f table
$A product https://www.alza.sk/iphone-15-128-gb-cierny-d7927612.htm
$A reviews 7927612 -n 50 -f table
```

Source: https://github.com/version-two/skills/tree/main/tools/alza (Rust).
