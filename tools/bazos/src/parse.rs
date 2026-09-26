use std::sync::LazyLock;

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use url::Url;

use crate::model::{Ad, Breadcrumb, Category, Listing, Price, PriceKind, SearchPage, Site};

type ParseResult<T> = Result<T, String>;

fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}

static TOTAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Zobrazen\w+ \d+-\d+ inzerát\w* z ([\d\s\u{a0}]+)").unwrap());
static DATE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[(\d{1,2})\.(\d{1,2})\.\s*(\d{4})\]").unwrap());
static ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/inzerat/(\d+)/").unwrap());
static AMOUNT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([\d ]+) (€|Kč)$").unwrap());
static LATLON_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/place/(-?[\d.]+),(-?[\d.]+)/").unwrap());

const NO_RESULTS_MARKERS: [&str; 2] = ["Hľadaniu nevyhovujú žiadne inzeráty", "Žádný inzerát nenalezen"];

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text_of(el: ElementRef) -> String {
    squash(&el.text().collect::<String>())
}

fn multiline_text(el: ElementRef) -> String {
    let raw: String = el.text().collect();
    let lines: Vec<String> = raw.lines().map(squash).collect();
    let mut out = Vec::new();
    let mut blank_run = 0;
    for line in lines {
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push(line);
    }
    out.join("\n").trim().to_string()
}

fn digits(s: &str) -> Option<u64> {
    let d: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    d.parse().ok()
}

pub fn parse_date(s: &str) -> Option<String> {
    let c = DATE_RE.captures(s)?;
    let day: u32 = c[1].parse().ok()?;
    let month: u32 = c[2].parse().ok()?;
    Some(format!("{}-{month:02}-{day:02}", &c[3]))
}

pub fn parse_price(raw: &str) -> Price {
    let raw = squash(raw);
    if let Some(c) = AMOUNT_RE.captures(&raw) {
        return Price {
            amount: digits(&c[1]).map(|v| v as i64),
            currency: Some(if &c[2] == "€" { "EUR" } else { "CZK" }.to_string()),
            kind: PriceKind::Amount,
            raw,
        };
    }
    let kind = match raw.as_str() {
        "Dohodou" => PriceKind::Negotiable,
        "Ponúknite" => PriceKind::Offer,
        "Zadarmo" | "Zdarma" => PriceKind::Free,
        "V texte" | "V textu" => PriceKind::InText,
        _ => PriceKind::Other,
    };
    Price { raw, kind, amount: None, currency: None }
}

fn ad_id(url: &str) -> ParseResult<u64> {
    ID_RE
        .captures(url)
        .and_then(|c| c[1].parse().ok())
        .ok_or_else(|| format!("no ad id in URL {url}"))
}

fn category_of(url: &Url) -> String {
    url.host_str().and_then(|h| h.split('.').next()).unwrap_or_default().to_string()
}

pub fn parse_search(html: &str, page_url: &Url) -> ParseResult<SearchPage> {
    let doc = Html::parse_document(html);
    let body_text: String = doc.root_element().text().collect();

    let total = match TOTAL_RE.captures(&body_text) {
        Some(c) => digits(&c[1]).ok_or("unreadable result counter")?,
        None if NO_RESULTS_MARKERS.iter().any(|m| body_text.contains(m)) => {
            return Ok(SearchPage { total: 0, listings: Vec::new() });
        }
        None => return Err("neither a result counter nor a no-results message found".into()),
    };

    let listings = doc
        .select(&sel("div.inzeraty"))
        .enumerate()
        .map(|(i, el)| parse_listing(el, page_url).map_err(|e| format!("listing #{}: {e}", i + 1)))
        .collect::<ParseResult<Vec<_>>>()?;

    if listings.is_empty() && total > 0 {
        return Err(format!("counter says {total} results but no listings were found"));
    }
    Ok(SearchPage { total, listings })
}

fn parse_listing(el: ElementRef, page_url: &Url) -> ParseResult<Listing> {
    let link = el.select(&sel("h2.nadpis a")).next().ok_or("missing title link")?;
    let href = link.value().attr("href").ok_or("title link has no href")?;
    let url = page_url.join(href).map_err(|e| format!("bad href {href}: {e}"))?;

    let meta = el.select(&sel("span.velikost10")).next().map(text_of).unwrap_or_default();
    let price_el = el.select(&sel("div.inzeratycena")).next().ok_or("missing price")?;
    let loc_parts: Vec<String> = el
        .select(&sel("div.inzeratylok"))
        .next()
        .ok_or("missing location")?
        .text()
        .map(squash)
        .filter(|s| !s.is_empty())
        .collect();

    Ok(Listing {
        id: ad_id(url.as_str())?,
        category: category_of(&url),
        title: text_of(link),
        snippet: el.select(&sel("div.popis")).next().map(multiline_text).unwrap_or_default(),
        price: parse_price(&text_of(price_el)),
        location: loc_parts.first().cloned(),
        postal_code: loc_parts.get(1).cloned(),
        views: el.select(&sel("div.inzeratyview")).next().and_then(|v| digits(&text_of(v))),
        date: parse_date(&meta),
        top: el.select(&sel("span.velikost10 span.ztop")).next().is_some(),
        thumbnail: el
            .select(&sel("img.obrazek"))
            .next()
            .and_then(|i| i.value().attr("src"))
            .filter(|src| !src.ends_with("/obrazky/empty.gif"))
            .map(str::to_string),
        url: url.to_string(),
    })
}

pub fn parse_ad(html: &str, url: &Url, site: Site) -> ParseResult<Ad> {
    let doc = Html::parse_document(html);
    let title_el = doc.select(&sel("h1.nadpisdetail")).next().ok_or("missing ad title (h1.nadpisdetail)")?;
    let header = doc.select(&sel("div.inzeratydetnadpis")).next().ok_or("missing ad header")?;
    let meta = header.select(&sel("span.velikost10")).next().map(text_of).unwrap_or_default();

    let mut ad = Ad {
        id: ad_id(url.as_str())?,
        url: url.to_string(),
        site,
        title: text_of(title_el),
        description: doc
            .select(&sel("div.popisdetail"))
            .next()
            .map(multiline_text)
            .ok_or("missing description (div.popisdetail)")?,
        price: None,
        date: parse_date(&meta),
        top: header.select(&sel("span.ztop")).next().is_some(),
        seller_name: None,
        location: None,
        postal_code: None,
        latitude: None,
        longitude: None,
        views: None,
        category: None,
        subcategory: None,
        images: doc
            .select(&sel("div.carousel img.carousel-cell-image"))
            .map(|i| {
                i.value()
                    .attr("data-flickity-lazyload")
                    .map(str::to_string)
                    .ok_or_else(|| "carousel image without data-flickity-lazyload".to_string())
            })
            .collect::<ParseResult<_>>()?,
    };

    for row in doc.select(&sel("td.listadvlevo tr")) {
        let cells: Vec<ElementRef> = row.select(&sel("td")).collect();
        let Some(label_cell) = cells.first() else { continue };
        let label = text_of(*label_cell);
        let value = || cells.last().map(|c| text_of(*c)).unwrap_or_default();
        match label.as_str() {
            "Meno:" | "Jméno:" => {
                ad.seller_name = row.select(&sel("b")).next().map(text_of).filter(|s| !s.is_empty());
            }
            "Lokalita:" => {
                for a in row.select(&sel("a")) {
                    let href = a.value().attr("href").unwrap_or_default();
                    if let Some(c) = LATLON_RE.captures(href) {
                        ad.latitude = c[1].parse().ok();
                        ad.longitude = c[2].parse().ok();
                        ad.postal_code = Some(text_of(a));
                    } else {
                        ad.location = Some(text_of(a));
                    }
                }
            }
            "Videlo:" | "Vidělo:" => ad.views = digits(&value()),
            "Cena:" => ad.price = Some(parse_price(&value())),
            _ => {}
        }
    }

    let crumbs: Vec<Breadcrumb> = doc
        .select(&sel("div.drobky a"))
        .skip(1)
        .filter_map(|a| {
            Some(Breadcrumb { name: text_of(a), url: a.value().attr("href")?.to_string() })
        })
        .collect();
    ad.category = crumbs.first().cloned();
    ad.subcategory = crumbs.get(1).cloned();

    if ad.price.is_none() {
        return Err("missing price row (Cena:)".into());
    }
    Ok(ad)
}

pub fn parse_categories(html: &str, site: Site) -> ParseResult<Vec<Category>> {
    let doc = Html::parse_document(html);
    let cats: Vec<Category> = doc
        .select(&sel("select[name=rubriky] option"))
        .filter_map(|o| {
            let slug = o.value().attr("value")?;
            (slug != "www").then(|| Category {
                category: slug.to_string(),
                slug: slug.to_string(),
                name: text_of(o),
                url: format!("https://{slug}.bazos.{}/", site.tld()),
            })
        })
        .collect();
    if cats.is_empty() {
        return Err("no categories in the rubriky selector".into());
    }
    Ok(cats)
}

pub fn parse_subcategories(html: &str, category_url: &Url) -> ParseResult<Vec<Category>> {
    let doc = Html::parse_document(html);
    let subs = doc
        .select(&sel("div.barvaleva a"))
        .map(|a| {
            let href = a.value().attr("href").ok_or("subcategory link without href")?;
            let url = category_url.join(href).map_err(|e| format!("bad href {href}: {e}"))?;
            Ok(Category {
                category: category_of(&url),
                slug: url.path().trim_matches('/').to_string(),
                name: text_of(a),
                url: url.to_string(),
            })
        })
        .collect::<ParseResult<Vec<_>>>()?;
    if subs.is_empty() {
        return Err("no subcategory menu (div.barvaleva) found".into());
    }
    Ok(subs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn prices() {
        let p = parse_price("  10 890 €");
        assert_eq!((p.kind, p.amount, p.currency.as_deref()), (PriceKind::Amount, Some(10890), Some("EUR")));
        let p = parse_price("1 000 Kč");
        assert_eq!((p.amount, p.currency.as_deref()), (Some(1000), Some("CZK")));
        assert_eq!(parse_price("Dohodou").kind, PriceKind::Negotiable);
        assert_eq!(parse_price("Zdarma").kind, PriceKind::Free);
        assert_eq!(parse_price("V texte").kind, PriceKind::InText);
        assert_eq!(parse_price("Ponúknite").kind, PriceKind::Offer);
        assert_eq!(parse_price("something new").kind, PriceKind::Other);
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date(" - TOP - [25.9. 2026]").as_deref(), Some("2026-09-25"));
        assert_eq!(parse_date("no date"), None);
    }

    #[test]
    fn www_search_sk() {
        let url = Url::parse("https://www.bazos.sk/search.php?hledat=iphone").unwrap();
        let page = parse_search(&fixture("search_www_sk.html"), &url).unwrap();
        assert_eq!(page.total, 6552);
        assert_eq!(page.listings.len(), 20);
        let first = &page.listings[0];
        assert_eq!(first.id, 195410373);
        assert_eq!(first.category, "auto");
        assert_eq!(first.title, "Volkswagen Tiguan 2.0 TDI");
        assert_eq!(first.price.amount, Some(10890));
        assert_eq!(first.location.as_deref(), Some("Prešov"));
        assert_eq!(first.postal_code.as_deref(), Some("080 01"));
        assert_eq!(first.views, Some(1334));
        assert_eq!(first.date.as_deref(), Some("2026-09-25"));
        assert!(first.top);
        assert!(first.snippet.starts_with("Predam:"));
    }

    #[test]
    fn category_search_resolves_relative_urls() {
        let url = Url::parse("https://mobil.bazos.sk/apple/?hledat=iphone").unwrap();
        let page = parse_search(&fixture("search_category_sk.html"), &url).unwrap();
        assert_eq!(page.listings.len(), 20);
        for l in &page.listings {
            assert!(l.url.starts_with("https://mobil.bazos.sk/inzerat/"), "{}", l.url);
            assert_eq!(l.category, "mobil");
        }
    }

    #[test]
    fn category_search_cz() {
        let url = Url::parse("https://mobil.bazos.cz/?hledat=iphone").unwrap();
        let page = parse_search(&fixture("search_category_cz.html"), &url).unwrap();
        assert_eq!(page.total, 931);
        assert_eq!(page.listings.len(), 20);
        assert!(page.listings.iter().all(|l| l.price.currency.as_deref() == Some("CZK") || l.price.amount.is_none()));
    }

    #[test]
    fn empty_search_is_zero_not_error() {
        let url = Url::parse("https://mobil.bazos.sk/apple/?hledat=zzzzqqqxxx").unwrap();
        let page = parse_search(&fixture("search_empty_sk.html"), &url).unwrap();
        assert_eq!(page.total, 0);
        assert!(page.listings.is_empty());
    }

    #[test]
    fn unknown_page_is_error() {
        let url = Url::parse("https://www.bazos.sk/").unwrap();
        assert!(parse_search("<html><body>maintenance</body></html>", &url).is_err());
    }

    #[test]
    fn ad_detail_sk() {
        let url = Url::parse("https://mobil.bazos.sk/inzerat/195868345/predam-iphone-14-pro-256-gb.php").unwrap();
        let ad = parse_ad(&fixture("ad_sk.html"), &url, Site::Sk).unwrap();
        assert_eq!(ad.id, 195868345);
        assert_eq!(ad.title, "PREDÁM iPhone 14 Pro 256 GB");
        assert_eq!(ad.price.as_ref().unwrap().amount, Some(350));
        assert_eq!(ad.seller_name.as_deref(), Some("Predajca"));
        assert_eq!(ad.postal_code.as_deref(), Some("841 04"));
        assert_eq!(ad.location.as_deref(), Some("Bratislava"));
        assert_eq!(ad.latitude, Some(48.153444));
        assert_eq!(ad.longitude, Some(17.060448));
        assert_eq!(ad.views, Some(372));
        assert_eq!(ad.date.as_deref(), Some("2026-09-25"));
        assert!(ad.top);
        assert_eq!(ad.images.len(), 8);
        assert_eq!(ad.category.as_ref().unwrap().name, "Mobily");
        assert_eq!(ad.subcategory.as_ref().unwrap().name, "Apple");
        assert!(ad.description.contains("Cena 350 € je fixná."));
        assert!(ad.description.contains('\n'));
    }

    #[test]
    fn ad_detail_cz() {
        let url = Url::parse("https://mobil.bazos.cz/inzerat/224038484/iphone-18-pro-256gb512gb-ledovcove-modra.php").unwrap();
        let ad = parse_ad(&fixture("ad_cz.html"), &url, Site::Cz).unwrap();
        assert_eq!(ad.title, "iPhone 18 Pro 256GB/512GB ledovcově modrá");
        assert_eq!(ad.price.as_ref().unwrap().currency.as_deref(), Some("CZK"));
        assert!(ad.seller_name.is_some());
        assert!(ad.views.is_some());
        assert!(ad.latitude.is_some());
        assert_eq!(ad.subcategory.as_ref().unwrap().url, "https://mobil.bazos.cz/apple/");
    }

    #[test]
    fn categories_and_subcategories() {
        let cats = parse_categories(&fixture("home_sk.html"), Site::Sk).unwrap();
        assert!(cats.iter().any(|c| c.slug == "mobil" && c.name == "Mobily"));
        assert!(cats.iter().all(|c| c.slug != "www"));
        let base = Url::parse("https://mobil.bazos.sk/").unwrap();
        let subs = parse_subcategories(&fixture("category_home_sk.html"), &base).unwrap();
        assert_eq!(subs[0].slug, "apple");
        assert_eq!(subs[0].category, "mobil");
        assert_eq!(subs[0].url, "https://mobil.bazos.sk/apple/");
        let gps = subs.iter().find(|s| s.slug == "gps").unwrap();
        assert_eq!(gps.category, "pc");
    }
}
