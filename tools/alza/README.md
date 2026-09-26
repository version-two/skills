# alza-cli

A command-line tool that searches Alza (alza.sk, alza.cz, alza.hu, alza.at, alza.de) through the JSON API of the official Alza Android app. JSON output is the default, so agents can use it directly.

Version Two s.r.o. – maintainer Mario Chamuty.

## Build

Prebuilt binaries: [releases](https://github.com/version-two/skills/releases) tagged `alza-v*`.

```sh
cargo build --release -p alza-cli   # from tools/, binary: target/release/alza(.exe)
cargo test -p alza-cli
```

## How it talks to Alza

The endpoints, request bodies and headers come from decompiling the Android app (`cz.alza.eshop` 2026.9.0, build 450). Cloudflare blocks generic clients with HTTP 403 but lets requests through that carry the app's User-Agent layout (`okhttp/5.2.1;<maker>/<model>;<android>;<locale>;2026.9.0;450;0;cz.alza.eshop`). No login or token is needed.

| Purpose | Endpoint |
|---|---|
| Text search | `POST /services/restservice.svc/v5/search` |
| Category listing | `POST /services/restservice.svc/v2/products?categoryId=ID` with `{"filterParameters": …}` |
| Category tree | `GET /services/restservice.svc/v1/category/ID?t=TYPE&p=TYPE_ID` |
| Filters (facets) | `GET /services/restservice.svc/v3/params/ID?type=TYPE&typeId=N&search=TEXT` |
| Product detail | `GET /api/router/legacy/catalog/product/ID` (redirects to `v13/product/ID`) |
| Review stats | link returned inside the product detail (`webapi.alza.cz/api/catalog/v2/commodities/ID/reviewStats`, needs the session tokens the link carries) |
| Reviews | link returned inside the review stats (`webapi.alza.cz/api/catalog/v2/commodities/ID/reviews`); the older link in the product detail is missing on some products that do have reviews |

The legacy RestService endpoints answer HTTP 200 even on failure and report it in-band as `{"err": n, "msg": ...}`; the tool turns every non-zero `err` into an error.

## Commands

### `alza search [QUERY]`

Searches by text, or with `--category` lists a category. The Alza API cannot scope a text search to a category, so the two are mutually exclusive; narrow a category with `--producer` and `--param` instead.

| Flag | Request field | Notes |
|---|---|---|
| `QUERY` | `searchTerm` | search text |
| `-c, --category REF` | `id`, `type`, `typeId` | `ID`, `ID:TYPE:TYPE_ID` (as printed in `ref`) or a category URL ending in `/<id>.htm` |
| `--price-min N` / `--price-max N` | `minPrice` / `maxPrice` | site currency |
| `-o, --sort` | `orderBy` | `recommended` (default), `bestselling` (categories only), `price-asc`, `price-desc`, `rating`, `newest`; checked against the sorts the API offers for the query |
| `--stock` | `availabilityType` | `any` (default), `anywhere` (in stock at Alza or a partner), `alza` (in stock at Alza) |
| `--branch ID` | `selectedBranches` | repeatable, needs `--stock alza`; showroom ids from `alza filters` |
| `--condition` | `commodityWears` | repeatable: `new`, `open-box` (unsealed or tested, full warranty), `used` (like new or used) |
| `--discounted` | `showOnlyActionCommodities` | promotions and discounts only |
| `--alza-plus` | `showOnlyAlzaPlusCommodities` | AlzaPlus+ products only |
| `--rated-4-plus` | `useRatingThreshold` | customer rating 4 stars or more |
| `--producer NAME\|ID` | `producers` | repeatable, checked against the live filter list |
| `--param SPEC` | `params` | repeatable, checked against the live filter list, see below |
| `-n, --limit N` | – | default 25; fetches as many 25-item pages as needed |
| `--offset N` | `page` | skips N listings in Alza's order |
| `--details` | – | also fetches every listing's product detail (adds `details: [...]`) |
| `--concurrency N` | – | parallel product fetches, default 4 |

`--param` takes `KEY=VALUE` for checkbox filters (repeat the flag to select several values) or `KEY=FROM..TO` for sliders, with either side optional. `KEY` is the filter id or its exact name; values are value ids / raw numbers or exact labels. Slider numbers use the filter's raw unit (RAM `16 GB` is `16384`), so labels are easier: `--param "Veľkosť operačnej pamäte RAM=16 GB.."`. Unknown producers, filters or values fail with the valid options listed.

### `alza product ID|URL...`

Full product detail: price (with and without VAT, original price when discounted), availability, warranty, part number, category and breadcrumbs, all images, every parameter group, documents, and review stats (average, counts, star distribution, recommendation and complaint rates). A product URL (`…-d<id>.htm`) also sets the site. The marketing description is a web page; its address is in `description_url`.

### `alza reviews ID|URL`

Customer reviews: rating, author, `reviewed_at` (ISO 8601 UTC timestamp), the reviewed `variant`, text, positives, negatives, verified purchase, translated, likes, images. `-n` (default 20) and `--offset` page through them. A product nobody reviewed returns `total: 0`.

Review stats (in `product`) have `average: null` while a product has no ratings, and `complaint_rate: null` until at least 30 customers bought it (`complaint_label` then says the rate is unknown).

### `alza categories [REF]`

Without `REF`, the top level. Each child has `name`, `ref`, `category {id, type, type_id}`, `section` and `url`. A `section` is a landing page that groups subcategories: browse it with `alza categories REF`, it has no product list of its own. Promotions appear as `ID:ACTION:N` refs and list like any category.

### `alza filters [QUERY] [-c REF]`

The filters Alza offers for a search text or a category: `producers` (id, name, count), `branches` (showrooms), `params` (id, group, name, `checkbox`/`slider`, values with label and count) and the price range.

### Global flags

`-s, --site cz|sk|hu|at|de` (default `sk`, or the site of a URL argument), `--lang cs|sk|en|de|hu` (response language, default the site's own; alza.at uses `de` like the app), `-f, --format json|jsonl|table`, `--delay-ms` (minimum gap between requests, default 300), `--timeout` (seconds, default 30).

`jsonl` prints one object per line: listings (or products with `--details`), products, reviews, category children, or filter entries tagged with `type`.

## Output

Prices: `{amount, currency, without_vat, original, raw}`. `currency` is `CZK` on alza.cz, `HUF` on alza.hu and `EUR` elsewhere. `original` is the pre-discount price when Alza shows one. `raw` is the display string. `price` is `null` when Alza shows no price, as for a product whose sale has ended (`can_buy: false`, availability e.g. "Verkauf beendet" on alza.de); when Alza shows a text instead of a price, such as "Cena nebola stanovená", it is in `price_note`. A price text that looks like an amount but comes without the numeric fields is a `parse_error`.

`total: 0` with an empty `listings` array means Alza reported no matches. Every other failure is an error.

## Errors

Errors go to stderr as `{"error": code, "message": ...}` with a non-zero exit code:

| Exit | Code | Meaning |
|---|---|---|
| 2 | `invalid_query`, `invalid_product` | bad arguments, unknown filter/producer/category, section used as a listing |
| 3 | `product_gone` | Alza reports the product does not exist |
| 4 | `http_error`, `transport_error`, `api_error` | network failure, non-2xx response (a 403 is usually Cloudflare) after 3 attempts for 429/5xx/timeouts, or an in-band API error |
| 5 | `parse_error` | the API response changed shape; nothing partial is returned |

## Examples

```sh
alza search "thinkpad" -o price-asc -n 10 -f table
alza search -c 18842920 --producer Lenovo --param "Veľkosť operačnej pamäte RAM=32 GB.." --param "Typ panela=OLED" --price-max 2500
alza search -c 18842920 --condition open-box --condition used --stock alza -o price-asc
alza search iphone -s cz --lang en --discounted
alza categories 18890188 -f table
alza filters -c 18842920 -f table
alza product https://www.alza.sk/iphone-15-128-gb-cierny-d7927612.htm
alza reviews 7927612 -n 50
```
