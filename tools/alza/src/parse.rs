use std::sync::LazyLock;

use regex::Regex;
use serde::de::DeserializeOwned;
use url::Url;

use crate::api;
use crate::model::{
    Branch, Category, CategoryLink, CategoryRef, Document, FilterKind, FilterParam, FilterValue, Filters, Listing,
    Page, Parameter, ParameterGroup, Price, Producer, Product, RatingBucket, Review, ReviewPage, ReviewStats, Site,
};

pub fn json<T: DeserializeOwned>(body: &str) -> Result<T, String> {
    serde_json::from_str(body).map_err(|e| e.to_string())
}

/// Parses Alza's display prices: `1 943,33 €`, `31 650,-`, `31,650,-` (en), `1.549 €`, `232 990 Ft`.
/// A final separator followed by exactly two digits is the decimal point; every other separator groups thousands.
pub fn parse_money(raw: &str) -> Result<f64, String> {
    let fail = || format!("unrecognised price `{raw}`");
    let trimmed = raw.trim();
    let body = trimmed.strip_suffix(",-").or_else(|| trimmed.strip_suffix(".-")).unwrap_or(trimmed);
    let mut digits = String::new();
    for c in body.chars() {
        match c {
            '0'..='9' | ',' | '.' => digits.push(c),
            c if c.is_whitespace() || c.is_alphabetic() || c == '€' => {}
            _ => return Err(fail()),
        }
    }
    let digits = digits.trim_matches(|c| c == ',' || c == '.');
    if !digits.chars().any(|c| c.is_ascii_digit()) {
        return Err(fail());
    }
    let (int_part, frac) = match digits.rfind([',', '.']) {
        Some(i) if digits.len() - i - 1 == 2 => (&digits[..i], Some(&digits[i + 1..])),
        _ => (digits, None),
    };
    let mut normal: String = int_part.chars().filter(char::is_ascii_digit).collect();
    if let Some(f) = frac {
        normal.push('.');
        normal.push_str(f);
    }
    normal.parse().map_err(|_| fail())
}

static ISO_TIMESTAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])T\d{2}:\d{2}:\d{2}(\.\d+)?Z$").unwrap());

static CATEGORY_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)/services/restservice\.svc/v1/category/(\d+)$").unwrap());
static SECTION_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)/api/catalog/v1/homePage/categories/(\d+)$").unwrap());

/// Maps an API category link to a category reference and whether it is a section (landing page).
/// SECTION is folded into CATEGORY: the category endpoint answers both identically.
pub fn category_from_href(href: &str) -> Option<(CategoryRef, bool)> {
    let url = Url::parse(href).ok()?;
    let path = url.path().trim_end_matches('/');
    if let Some(c) = SECTION_PATH.captures(path) {
        return Some((CategoryRef::plain(c[1].parse().ok()?), true));
    }
    let id: u64 = CATEGORY_PATH.captures(path)?[1].parse().ok()?;
    let mut kind = "CATEGORY".to_string();
    let mut kind_id = 0;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "t" => kind = v.to_ascii_uppercase(),
            "p" => kind_id = v.parse().ok()?,
            _ => {}
        }
    }
    let section = kind == "SECTION";
    if section {
        kind = "CATEGORY".into();
    }
    Some((CategoryRef { id, kind, kind_id }, section))
}

fn link(name: String, href: &str, section: bool, web_url: Option<String>) -> CategoryLink {
    match category_from_href(href) {
        Some((c, from_href)) => CategoryLink {
            name,
            reference: Some(c.to_arg()),
            category: Some(c),
            section: section || from_href,
            url: web_url,
        },
        None => CategoryLink { name, reference: None, category: None, section: false, url: Some(href.to_string()) },
    }
}

fn category_item(item: api::CategoryItem) -> CategoryLink {
    link(item.name, &item.link.href, item.ltp, item.url)
}

fn breadcrumb(b: api::Breadcrumb) -> Result<CategoryLink, String> {
    let c = b.category;
    let href = c
        .meta
        .or(c.client_action)
        .ok_or_else(|| format!("breadcrumb `{}` has no link", c.name))?
        .href;
    if category_from_href(&href).is_none() {
        return Err(format!("breadcrumb `{}` links to unknown target {href}", c.name));
    }
    Ok(link(c.name, &href, false, None))
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Products without a current price carry no numeric price fields: discontinued ones show no price at all, others a
/// text such as "Cena nebola stanovená" (returned as the note). A leftover root `priceNoCurrency` is not reported.
fn price(item: &api::ListItem, site: Site) -> Result<(Option<Price>, Option<String>), String> {
    let info = &item.price_info_v2;
    let (raw, without_vat) = match (&info.price_with_vat, info.price_without_vat_no_currency) {
        (None, None) => return Ok((None, None)),
        (Some(text), None) if parse_money(text).is_err() => return Ok((None, non_empty(Some(text.clone())))),
        (Some(raw), Some(without_vat)) => (raw.clone(), without_vat),
        _ => return Err(format!("product {} has partial price data", item.id)),
    };
    let amount = item.price_no_currency.ok_or_else(|| format!("product {} has a display price but no amount", item.id))?;
    let original = info.compare_price.as_deref().map(parse_money).transpose()?;
    Ok((Some(Price { amount, currency: site.currency(), without_vat, original, raw }), None))
}

pub fn listing(item: api::ListItem, site: Site) -> Result<Listing, String> {
    let (price, price_note) = price(&item, site)?;
    Ok(Listing {
        id: item.id,
        code: item.code,
        name: item.name,
        url: item.url,
        spec: non_empty(item.spec),
        image: non_empty(item.img),
        price,
        price_note,
        availability: non_empty(item.avail),
        can_buy: item.can_buy,
        rating: item.rating,
        rating_count: item.rating_count,
        promo: non_empty(item.action_name),
    })
}

fn listings(items: Vec<api::ListItem>, site: Site) -> Result<Vec<Listing>, String> {
    items.into_iter().map(|i| listing(i, site)).collect()
}

pub fn parse_search(body: &str, site: Site) -> Result<(Page, Option<String>), String> {
    let r: api::SearchResponse = json(body)?;
    let page = Page {
        total: r.total,
        has_next: r.has_next,
        sorts: r.sorts.iter().map(|s| s.sort).collect(),
        listings: listings(r.data2, site)?,
        related_categories: r.data1.into_iter().map(category_item).collect(),
    };
    Ok((page, r.share_url))
}

pub fn parse_products(body: &str, site: Site) -> Result<(Page, Option<String>), String> {
    let r: api::ProductsResponse = json(body)?;
    let page = Page {
        total: r.total,
        has_next: r.has_next,
        sorts: Vec::new(),
        listings: listings(r.data, site)?,
        related_categories: Vec::new(),
    };
    Ok((page, r.share_url))
}

pub struct CategoryPage {
    pub category: Category,
    pub sorts: Vec<i64>,
}

pub fn parse_category(body: &str, requested: &CategoryRef) -> Result<CategoryPage, String> {
    let r: api::CategoryResponse = json(body)?;
    let breadcrumbs = r.breadcrumbs.into_iter().map(breadcrumb).collect::<Result<_, _>>()?;
    let name = if r.cat_name.trim().is_empty() { r.current.name } else { r.cat_name };
    Ok(CategoryPage {
        sorts: r.current.sorts.iter().map(|s| s.sort).collect(),
        category: Category {
            name,
            reference: requested.to_arg(),
            category: requested.clone(),
            section: r.ltp || r.current.ltp,
            url: r.current.url,
            breadcrumbs,
            children: r.data.into_iter().map(category_item).collect(),
        },
    })
}

pub fn parse_filters(body: &str) -> Result<Filters, String> {
    let r: api::ParamsResponse = json(body)?;
    let mut params = Vec::new();
    for g in r.params.into_iter().flat_map(|p| p.groups) {
        for p in g.params {
            let kind = match p.render_type.as_str() {
                "Checkbox" => FilterKind::Checkbox,
                "Slider" => FilterKind::Slider,
                other => return Err(format!("filter `{}` has unknown render type `{other}`", p.name)),
            };
            params.push(FilterParam {
                id: p.t_id,
                group: g.name.clone(),
                name: p.name,
                kind,
                values: p
                    .values
                    .into_iter()
                    .map(|v| FilterValue { value: v.v, label: v.desc, count: v.cnt })
                    .collect(),
            });
        }
    }
    let steps: Vec<f64> = r.prices.iter().map(|p| p.k).collect();
    Ok(Filters {
        price_min: steps.iter().copied().reduce(f64::min),
        price_max: steps.iter().copied().reduce(f64::max),
        producers: r.producers.into_iter().map(|p| Producer { id: p.v, name: p.desc, count: p.cnt }).collect(),
        branches: r.branches.into_iter().map(|b| Branch { id: b.id, name: b.name }).collect(),
        params,
    })
}

pub struct ParsedDetail {
    pub product: Product,
    pub review_stats_url: Option<String>,
}

pub fn parse_detail(body: &str, site: Site) -> Result<ParsedDetail, String> {
    let r: api::DetailResponse = json(body)?;
    let d = r.data;
    let (price, price_note) = price(&d.item, site)?;
    let breadcrumbs = d.breadcrumb.unwrap_or_default().into_iter().map(breadcrumb).collect::<Result<_, _>>()?;
    let category = match (d.category_id, d.category_name) {
        (Some(id), Some(name)) if id > 0 => {
            let c = CategoryRef::plain(id);
            Some(CategoryLink { name, reference: Some(c.to_arg()), category: Some(c), section: false, url: None })
        }
        _ => None,
    };
    let product = Product {
        id: d.item.id,
        site,
        code: d.item.code,
        name: d.item.name,
        url: d.item.url,
        spec: non_empty(d.item.spec),
        price,
        price_note,
        availability: non_empty(d.item.avail),
        availability_note: non_empty(d.avail_postfix),
        in_stock: d.is_in_stock,
        can_buy: d.item.can_buy,
        warranty: non_empty(d.warranty),
        producer_id: d.producer_id.filter(|&p| p > 0),
        part_number: non_empty(d.catalog_number),
        category,
        breadcrumbs,
        rating: d.item.rating,
        reviews: None,
        promo: non_empty(d.item.action_name),
        images: d.imgs.unwrap_or_default().into_iter().map(|i| i.orig_url).collect(),
        parameters: d
            .parameter_groups
            .unwrap_or_default()
            .into_iter()
            .map(|g| ParameterGroup {
                name: g.name,
                parameters: g
                    .params
                    .into_iter()
                    .map(|p| Parameter { name: p.name, values: p.values.into_iter().map(|v| v.desc).collect() })
                    .collect(),
            })
            .collect(),
        documents: d.links.unwrap_or_default().into_iter().map(|l| Document { name: l.name, url: l.url }).collect(),
        description_url: non_empty(d.desc_page_url),
    };
    Ok(ParsedDetail { product, review_stats_url: d.review_stats.map(|l| l.href) })
}

/// Returns the stats and the link to the review list, which only the stats response carries for every product.
pub fn parse_review_stats(body: &str) -> Result<(ReviewStats, Option<String>), String> {
    let r: api::ReviewStats = json(body)?;
    let mut distribution: Vec<RatingBucket> =
        r.ratings.into_iter().map(|b| RatingBucket { stars: b.value, count: b.count }).collect();
    distribution.sort_by_key(|b| b.stars);
    let stats = ReviewStats {
        average: r.rating_average,
        rating_count: r.rating_count,
        review_count: r.review_count,
        recommendation_rate: r.recommendation_rate,
        distribution,
        complaint_rate: r.complaint.as_ref().and_then(|c| c.rate),
        complaint_label: r.complaint.and_then(|c| non_empty(c.description)),
        purchases: non_empty(r.purchase_count_formatted),
    };
    Ok((stats, r.commodity_reviews_action.map(|l| l.href)))
}

pub fn parse_reviews(body: &str) -> Result<ReviewPage, String> {
    let r: api::ReviewsResponse = json(body)?;
    let reviews = r
        .value
        .into_iter()
        .map(|i| {
            if !ISO_TIMESTAMP.is_match(&i.review_date) {
                return Err(format!("unrecognised review date `{}`", i.review_date));
            }
            Ok(Review {
                rating: i.rating,
                author: i.name,
                reviewed_at: i.review_date,
                variant: non_empty(i.commodity_name),
                text: i.description.trim().to_string(),
                positives: i.positives.into_iter().map(|s| s.trim().to_string()).collect(),
                negatives: i.negatives.into_iter().map(|s| s.trim().to_string()).collect(),
                verified_purchase: i.verified_purchase_tag.is_some(),
                translated: i.is_translated,
                likes: i.like_count,
                images: i.images.into_iter().map(|im| im.image_url).collect(),
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(ReviewPage { total: r.paging.size, reviews })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn money_formats_of_every_site() {
        assert_eq!(parse_money("1\u{a0}987,78\u{a0}€"), Ok(1987.78));
        assert_eq!(parse_money("624,90 €"), Ok(624.9));
        assert_eq!(parse_money("33\u{a0}317,-"), Ok(33317.0));
        assert_eq!(parse_money("33,317,-"), Ok(33317.0));
        assert_eq!(parse_money("1.620\u{a0}€"), Ok(1620.0));
        assert_eq!(parse_money("261\u{a0}990\u{a0}Ft"), Ok(261990.0));
        assert_eq!(parse_money("1,987.78 €"), Ok(1987.78));
        assert_eq!(parse_money("799 €"), Ok(799.0));
        assert!(parse_money("zadarmo").is_err());
        assert!(parse_money("od 5 € / mesiac").is_err());
    }

    #[test]
    fn category_hrefs() {
        let (c, section) =
            category_from_href("https://m.alza.sk/services/restservice.svc/v1/category/18851729?t=CATEGORY").unwrap();
        assert_eq!((c.to_arg(), section), ("18851729".to_string(), false));
        let (c, _) = category_from_href("https://www.alza.sk/services/restservice.svc/v1/category/1?t=ACTION&p=17").unwrap();
        assert_eq!(c, CategoryRef { id: 1, kind: "ACTION".into(), kind_id: 17 });
        assert_eq!(c.to_arg(), "1:ACTION:17");
        let (c, section) =
            category_from_href("https://m.alza.sk/services/restservice.svc/v1/category/18890259?t=SECTION").unwrap();
        assert_eq!((c.to_arg(), section), ("18890259".to_string(), true));
        let (c, section) =
            category_from_href("https://www.alza.sk/api/catalog/v1/homePage/categories/18890188?pgri=p__1&ui=u__1").unwrap();
        assert_eq!((c.to_arg(), section), ("18890188".to_string(), true));
        assert!(category_from_href("https://www.alza.sk/koniec-podpory-pre-windows-10").is_none());
    }

    #[test]
    fn search_page() {
        let (page, share) = parse_search(&fixture("search_sk.json"), Site::Sk).unwrap();
        assert_eq!((page.total, page.has_next, page.listings.len()), (3647, true, 25));
        assert_eq!(page.sorts, vec![0, 1, 2, 6, 5]);
        assert_eq!(share.as_deref(), Some("https://m.alza.sk/search.htm?exps=notebook"));
        let l = &page.listings[0];
        assert_eq!((l.id, l.code.as_str(), l.name.as_str()), (13141863, "ADC454p20e", "Dell Pro 15 Essential PV15250"));
        assert_eq!(l.price, Some(Price {
            amount: 709.0,
            currency: "EUR",
            without_vat: 576.4228,
            original: None,
            raw: "709\u{a0}€".into()
        }));
        assert!(l.url.starts_with("https://www.alza.sk/") && l.url.ends_with("-d13141863.htm"));
        assert_eq!(page.related_categories.len(), 15);
        assert_eq!(page.related_categories[0].name, "Copilot+ PC");
        assert_eq!(page.related_categories[0].reference.as_deref(), Some("18911035"));
    }

    #[test]
    fn empty_search_is_a_real_zero() {
        let (page, _) = parse_search(&fixture("search_empty_sk.json"), Site::Sk).unwrap();
        assert_eq!((page.total, page.has_next, page.listings.len()), (0, false, 0));
    }

    #[test]
    fn discount_prices_on_every_site() {
        let cases = [
            ("products_discount_sk.json", Site::Sk, 12937013, 1943.33, 1987.78, "EUR"),
            ("search_discount_cz.json", Site::Cz, 13413767, 31650.0, 33317.0, "CZK"),
            ("search_discount_cz_en.json", Site::Cz, 13413767, 31650.0, 33317.0, "CZK"),
            ("search_discount_hu.json", Site::Hu, 13150721, 232990.0, 261990.0, "HUF"),
            ("search_discount_de.json", Site::De, 13448136, 1549.0, 1620.0, "EUR"),
        ];
        for (file, site, id, amount, original, currency) in cases {
            let body = fixture(file);
            let (page, _) = if file.starts_with("products") {
                parse_products(&body, site).unwrap()
            } else {
                parse_search(&body, site).unwrap()
            };
            let l = page.listings.iter().find(|l| l.id == id).unwrap_or_else(|| panic!("{file}: {id} missing"));
            let p = l.price.as_ref().unwrap();
            assert_eq!((p.amount, p.original, p.currency), (amount, Some(original), currency), "{file}");
        }
    }

    #[test]
    fn discontinued_product_has_no_price() {
        let (page, _) = parse_search(&fixture("search_discontinued_de.json"), Site::De).unwrap();
        let l = page.listings.iter().find(|l| l.id == 217600).unwrap();
        assert_eq!((l.price.as_ref(), l.can_buy, l.availability.as_deref()), (None, false, Some("Verkauf beendet")));
        assert!(page.listings.iter().filter(|l| l.id != 217600).all(|l| l.price.is_some()));

        let d = parse_detail(&fixture("product_discontinued_de.json"), Site::De).unwrap();
        assert_eq!((d.product.price, d.product.in_stock, d.product.can_buy), (None, false, false));
        assert_eq!(d.product.name, "Parrot AR.Drone Fixing Tape");
    }

    #[test]
    fn price_not_set_keeps_the_note() {
        let (page, _) = parse_search(&fixture("search_price_note_sk.json"), Site::Sk).unwrap();
        let l = page.listings.iter().find(|l| l.id == 6258665).unwrap();
        assert_eq!((l.price.as_ref(), l.price_note.as_deref(), l.can_buy), (None, Some("Cena nebola stanovená"), false));

        let d = parse_detail(&fixture("product_price_note_sk.json"), Site::Sk).unwrap();
        assert_eq!((d.product.price.as_ref(), d.product.price_note.as_deref()), (None, Some("Cena nebola stanovená")));
    }

    #[test]
    fn partial_price_data_is_an_error() {
        let body = fixture("search_sk.json").replacen("\"priceWithoutVatNoCurrency\": 576.4228", "\"priceWithoutVatNoCurrency\": null", 1);
        assert!(parse_search(&body, Site::Sk).unwrap_err().contains("partial price data"));
    }

    #[test]
    fn category_listing() {
        let (page, _) = parse_products(&fixture("products_sk.json"), Site::Sk).unwrap();
        assert_eq!((page.total, page.has_next, page.listings.len()), (3647, true, 25));
    }

    #[test]
    fn section_category() {
        let page = parse_category(&fixture("category_section_sk.json"), &CategoryRef::plain(18890188)).unwrap();
        let c = page.category;
        assert_eq!((c.name.as_str(), c.section, c.reference.as_str()), ("Počítače a notebooky", true, "18890188"));
        assert_eq!(page.sorts, vec![0, 7, 1, 2, 6, 5]);
        let refs: Vec<(&str, Option<&str>, bool)> =
            c.children.iter().map(|x| (x.name.as_str(), x.reference.as_deref(), x.section)).collect();
        assert!(refs.contains(&("AlzaPlus+ zľavy", Some("18890188:ACTION:17"), false)));
        assert!(refs.contains(&("Notebooky", Some("18842920"), false)));
        assert!(refs.contains(&("Tlačiarne a skenery", Some("18851089"), true)));
        let wizard = c.children.iter().find(|x| x.name == "Koniec podpory Windows 10").unwrap();
        assert_eq!((wizard.reference.as_deref(), wizard.url.as_deref()), (None, Some("https://www.alza.sk/koniec-podpory-pre-windows-10")));
        assert_eq!(c.breadcrumbs.len(), 1);
    }

    #[test]
    fn root_category() {
        let page = parse_category(&fixture("category_root_sk.json"), &CategoryRef::plain(0)).unwrap();
        assert_eq!(page.category.children.len(), 25);
        let mobiles = page.category.children.iter().find(|x| x.name == "Mobily, smart hodinky, tablety").unwrap();
        assert_eq!((mobiles.reference.as_deref(), mobiles.section), (Some("18890259"), true));
    }

    #[test]
    fn category_filters() {
        let f = parse_filters(&fixture("params_category_sk.json")).unwrap();
        assert_eq!(f.producers.len(), 20);
        assert_eq!(f.producers[0], Producer { id: 1627, name: "Apple".into(), count: 1267 });
        assert_eq!(f.branches.len(), 2);
        assert_eq!(f.params.len(), 65);
        let ram = f.params.iter().find(|p| p.id == 70).unwrap();
        assert_eq!((ram.kind, ram.name.as_str()), (FilterKind::Slider, "Veľkosť operačnej pamäte RAM"));
        let panel = f.params.iter().find(|p| p.id == 102).unwrap();
        assert_eq!(panel.kind, FilterKind::Checkbox);
        assert!(panel.values.iter().any(|v| v.label == "OLED" && v.value == 915.0));
        assert!(f.price_min.unwrap() < f.price_max.unwrap());
    }

    #[test]
    fn search_filters() {
        let f = parse_filters(&fixture("params_search_sk.json")).unwrap();
        assert_eq!(f.producers, vec![Producer { id: 1776, name: "Lenovo".into(), count: 104 }]);
        let series = f.params.iter().find(|p| p.id == 13376).unwrap();
        assert!(series.values.iter().any(|v| v.label == "Lenovo ThinkPad E"));
    }

    #[test]
    fn product_detail() {
        let d = parse_detail(&fixture("product_sk.json"), Site::Sk).unwrap();
        let p = &d.product;
        assert_eq!((p.id, p.code.as_str(), p.name.as_str()), (7927612, "RI045b1", "iPhone 15 128 GB čierny"));
        assert_eq!(p.price.as_ref().map(|x| (x.amount, x.currency)), Some((799.0, "EUR")));
        assert!(p.in_stock && p.can_buy);
        assert_eq!(p.part_number.as_deref(), Some("mtp03sx/a"));
        assert_eq!(p.producer_id, Some(1627));
        assert_eq!(p.images.len(), 11);
        assert!(p.images.iter().all(|i| i.starts_with("https://image.alza.cz/")));
        assert_eq!(p.parameters.len(), 14);
        assert_eq!(p.parameters[0].parameters[1], Parameter { name: "Úložisko".into(), values: vec!["128 GB".into()] });
        assert_eq!(p.breadcrumbs.len(), 5);
        assert!(p.breadcrumbs[0].section);
        assert_eq!(p.documents.len(), 1);
        assert!(d.review_stats_url.unwrap().contains("/reviewStats"));
    }

    #[test]
    fn review_stats() {
        let (s, reviews_url) = parse_review_stats(&fixture("review_stats_sk.json")).unwrap();
        assert_eq!((s.average, s.rating_count, s.review_count), (Some(4.9), 1168, 248));
        assert_eq!(s.distribution.iter().map(|b| b.stars).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
        assert_eq!(s.distribution.iter().map(|b| b.count).sum::<u64>(), 1168);
        assert!(s.complaint_rate.is_some());
        assert!(reviews_url.unwrap().contains("/v2/commodities/7927612/reviews"));
    }

    #[test]
    fn review_stats_of_an_unrated_product() {
        let (s, reviews_url) = parse_review_stats(&fixture("review_stats_unrated_sk.json")).unwrap();
        assert_eq!((s.average, s.rating_count, s.review_count, s.complaint_rate), (None, 0, 0, None));
        assert_eq!(s.complaint_label.as_deref(), Some("neznáma reklamovanosť"));
        assert!(reviews_url.is_some());
    }

    #[test]
    fn reviews_in_every_language() {
        let page = parse_reviews(&fixture("reviews_sk.json")).unwrap();
        assert_eq!((page.total, page.reviews.len()), (248, 20));
        let r = &page.reviews[0];
        assert_eq!(
            (r.rating, r.author.as_str(), r.reviewed_at.as_str(), r.variant.as_deref()),
            (5.0, "Reviewer 1", "2026-06-14T15:48:14Z", Some("iPhone 15 128 GB čierny"))
        );
        assert!(r.verified_purchase);
        for (file, total, at) in
            [("reviews_hu.json", 188, "2024-11-03T10:09:00.023Z"), ("reviews_cz_en.json", 200, "2026-09-12T08:16:32.967Z")]
        {
            let page = parse_reviews(&fixture(file)).unwrap();
            assert_eq!((page.total, page.reviews.len()), (total, 20), "{file}");
            assert_eq!(page.reviews[0].reviewed_at, at, "{file}");
        }
    }

    #[test]
    fn rejects_unknown_review_date_format() {
        let body = fixture("reviews_sk.json").replacen("2026-06-14T15:48:14Z", "14. 6. 2026", 1);
        assert!(parse_reviews(&body).unwrap_err().contains("14. 6. 2026"));
    }

    #[test]
    fn api_error_envelope() {
        let body = r#"{"data":null,"err":1,"msg":"Produkt pre zadané id neexistuje."}"#;
        let env: api::Envelope = json(body).unwrap();
        assert_eq!((env.err, env.msg.as_deref()), (1, Some("Produkt pre zadané id neexistuje.")));
        assert!(parse_detail(body, Site::Sk).is_err());
    }
}
