use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};
use url::Url;

use crate::error::Error;
use crate::model::{CategoryRef, Condition, FilterKind, FilterParam, Filters, Site, Sort, Stock};

pub const PAGE_SIZE: u64 = 25;

static CATEGORY_ARG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)(?::([A-Za-z]+):(\d+))?$").unwrap());
static CATEGORY_URL_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/(\d+)\.htm$").unwrap());
static PRODUCT_URL_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-d(\d+)\.htm$").unwrap());

fn site_for(url: &Url, explicit: Option<Site>, raw: &str) -> Result<Site, String> {
    let from_url = url.host_str().and_then(Site::from_host).ok_or_else(|| format!("`{raw}` is not an alza.cz/sk/hu/at/de URL"))?;
    match explicit {
        Some(s) if s != from_url => {
            Err(format!("`{raw}` belongs to alza.{}, but --site {} was given", from_url.tld(), s.tld()))
        }
        _ => Ok(from_url),
    }
}

/// Accepts `ID`, `ID:TYPE:TYPE_ID` (as printed in `ref`) or a category page URL ending in `/<id>.htm`.
pub fn parse_category_arg(raw: &str, explicit: Option<Site>) -> Result<(CategoryRef, Site), Error> {
    let raw = raw.trim();
    let default_site = explicit.unwrap_or(Site::Sk);
    if let Some(c) = CATEGORY_ARG.captures(raw) {
        let id = c[1].parse().map_err(|_| Error::InvalidQuery(format!("category id out of range: `{raw}`")))?;
        let category = match (c.get(2), c.get(3)) {
            (Some(kind), Some(kind_id)) => CategoryRef {
                id,
                kind: kind.as_str().to_ascii_uppercase(),
                kind_id: kind_id.as_str().parse().map_err(|_| Error::InvalidQuery(format!("bad type id in `{raw}`")))?,
            },
            _ => CategoryRef::plain(id),
        };
        return Ok((category, default_site));
    }
    let url = Url::parse(raw).map_err(|_| {
        Error::InvalidQuery(format!("category must be an id, `ID:TYPE:TYPE_ID` or an Alza category URL, got `{raw}`"))
    })?;
    let site = site_for(&url, explicit, raw).map_err(Error::InvalidQuery)?;
    let id = CATEGORY_URL_PATH
        .captures(url.path())
        .and_then(|c| c[1].parse().ok())
        .ok_or_else(|| {
            Error::InvalidQuery(format!(
                "`{raw}` has no category id in its path (expected `/<id>.htm`); find the ref with `alza categories`"
            ))
        })?;
    Ok((CategoryRef::plain(id), site))
}

/// Accepts a numeric product id or a product page URL ending in `-d<id>.htm`.
pub fn parse_product_arg(raw: &str, explicit: Option<Site>) -> Result<(u64, Site), Error> {
    let raw = raw.trim();
    if let Ok(id) = raw.parse::<u64>() {
        return Ok((id, explicit.unwrap_or(Site::Sk)));
    }
    let url = Url::parse(raw).map_err(|_| Error::InvalidProduct(raw.to_string()))?;
    if url.host_str().and_then(Site::from_host).is_none() {
        return Err(Error::InvalidProduct(raw.to_string()));
    }
    let site = site_for(&url, explicit, raw).map_err(Error::InvalidQuery)?;
    let id = PRODUCT_URL_PATH
        .captures(url.path())
        .and_then(|c| c[1].parse().ok())
        .ok_or_else(|| Error::InvalidProduct(raw.to_string()))?;
    Ok((id, site))
}

#[derive(Debug, Clone)]
pub struct Query {
    pub site: Site,
    pub text: Option<String>,
    pub category: Option<CategoryRef>,
    pub price_min: Option<f64>,
    pub price_max: Option<f64>,
    pub sort: Sort,
    pub stock: Stock,
    pub branches: Vec<i64>,
    pub conditions: Vec<Condition>,
    pub discounted: bool,
    pub alza_plus: bool,
    pub rated_4_plus: bool,
    pub producers: Vec<String>,
    pub params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamFilter {
    Values(Vec<f64>),
    Range(Option<f64>, Option<f64>),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resolved {
    pub producers: Vec<i64>,
    pub params: BTreeMap<i64, ParamFilter>,
}

impl Query {
    pub fn validate(&self) -> Result<(), Error> {
        let text = self.text.as_deref().map(str::trim).filter(|t| !t.is_empty());
        match (text, &self.category) {
            (None, None) => {
                return Err(Error::InvalidQuery("give a search text or --category".into()));
            }
            (Some(_), Some(_)) => {
                return Err(Error::InvalidQuery(
                    "search text and --category cannot be combined: the Alza API does not scope a text search to a \
                     category. Search by text, or list the category and narrow it with --producer/--param"
                        .into(),
                ));
            }
            _ => {}
        }
        if let (Some(min), Some(max)) = (self.price_min, self.price_max)
            && min > max
        {
            return Err(Error::InvalidQuery(format!("--price-min {min} is above --price-max {max}")));
        }
        if !self.branches.is_empty() && self.stock != Stock::Alza {
            return Err(Error::InvalidQuery("--branch needs --stock alza".into()));
        }
        Ok(())
    }

    pub fn needs_filters(&self) -> bool {
        !self.producers.is_empty() || !self.params.is_empty() || !self.branches.is_empty()
    }

    pub fn body(&self, resolved: &Resolved, page: u64) -> Value {
        let (id, kind, kind_id) = match &self.category {
            Some(c) => (c.id, c.kind.as_str(), c.kind_id),
            None => (0, "", 0),
        };
        let wears: Vec<i64> = self.conditions.iter().flat_map(|c| c.wear_ids().iter().copied()).collect();
        let params: Vec<Value> = resolved
            .params
            .iter()
            .map(|(t_id, f)| match f {
                ParamFilter::Values(v) => json!({"tId": t_id, "v": v, "vFrom": null, "vTo": null}),
                ParamFilter::Range(from, to) => json!({"tId": t_id, "v": null, "vFrom": from, "vTo": to}),
            })
            .collect();
        json!({
            "id": id,
            "type": kind,
            "typeId": kind_id,
            "orderBy": self.sort.code(),
            "page": page,
            "availabilityType": self.stock.code(),
            "selectedBranches": if self.stock == Stock::Alza { self.branches.clone() } else { Vec::new() },
            "commodityWears": if wears.is_empty() { Value::Null } else { json!(wears) },
            "sendPrices": true,
            "minPrice": self.price_min,
            "maxPrice": self.price_max,
            "params": params,
            "areas": null,
            "latitude": null,
            "longitude": null,
            "producers": resolved.producers,
            "searchTerm": self.text.as_deref().map(str::trim).unwrap_or(""),
            "useRatingThreshold": self.rated_4_plus,
            "showOnlyActionCommodities": self.discounted,
            "showOnlyAlzaPlusCommodities": self.alza_plus,
        })
    }
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn listed<T>(items: &[T], label: impl Fn(&T) -> String) -> String {
    const SHOWN: usize = 40;
    let mut out: Vec<String> = items.iter().take(SHOWN).map(label).collect();
    if items.len() > SHOWN {
        out.push(format!("… {} more (see `alza filters`)", items.len() - SHOWN));
    }
    out.join(", ")
}

fn resolve_producer(raw: &str, filters: &Filters) -> Result<i64, Error> {
    let hit = match raw.trim().parse::<i64>() {
        Ok(id) => filters.producers.iter().find(|p| p.id == id),
        Err(_) => filters.producers.iter().find(|p| norm(&p.name) == norm(raw)),
    };
    hit.map(|p| p.id).ok_or_else(|| {
        Error::InvalidQuery(format!(
            "unknown producer `{raw}` for this search; valid: {}",
            listed(&filters.producers, |p| format!("{} ({})", p.name, p.id))
        ))
    })
}

fn find_param<'a>(key: &str, filters: &'a Filters) -> Result<&'a FilterParam, Error> {
    let key = key.trim();
    let hits: Vec<&FilterParam> = match key.parse::<i64>() {
        Ok(id) => filters.params.iter().filter(|p| p.id == id).collect(),
        Err(_) => filters.params.iter().filter(|p| norm(&p.name) == norm(key)).collect(),
    };
    match hits.as_slice() {
        [one] => Ok(one),
        [] => Err(Error::InvalidQuery(format!(
            "unknown filter `{key}` for this search; valid: {}",
            listed(&filters.params, |p| format!("{} ({})", p.name, p.id))
        ))),
        many => Err(Error::InvalidQuery(format!(
            "filter name `{key}` is ambiguous, use the id: {}",
            listed(many, |p| format!("{} / {} ({})", p.group, p.name, p.id))
        ))),
    }
}

fn resolve_value(raw: &str, param: &FilterParam) -> Result<f64, Error> {
    let raw = raw.trim();
    let hit = match raw.parse::<f64>() {
        Ok(n) if param.kind == FilterKind::Slider => return Ok(n),
        Ok(n) => param.values.iter().find(|v| v.value == n),
        Err(_) => param.values.iter().find(|v| norm(&v.label) == norm(raw)),
    };
    hit.map(|v| v.value).ok_or_else(|| {
        Error::InvalidQuery(format!(
            "unknown value `{raw}` for filter `{}` ({}); valid: {}",
            param.name,
            param.id,
            listed(&param.values, |v| format!("{} ({})", v.label, v.value))
        ))
    })
}

/// `KEY=VALUE` selects a checkbox value (repeat the flag to select several); `KEY=FROM..TO` bounds a slider,
/// either side optional. KEY is the filter id or its exact name, values are ids/numbers or exact labels.
pub fn resolve(query: &Query, filters: &Filters) -> Result<Resolved, Error> {
    let mut out = Resolved::default();
    for raw in &query.producers {
        let id = resolve_producer(raw, filters)?;
        if !out.producers.contains(&id) {
            out.producers.push(id);
        }
    }
    for raw in &query.params {
        let (key, value) = raw
            .split_once('=')
            .ok_or_else(|| Error::InvalidQuery(format!("--param must be KEY=VALUE or KEY=FROM..TO, got `{raw}`")))?;
        let param = find_param(key, filters)?;
        match param.kind {
            FilterKind::Checkbox => {
                if value.contains("..") {
                    return Err(Error::InvalidQuery(format!(
                        "filter `{}` is a checkbox list; pass one value per --param, not a range",
                        param.name
                    )));
                }
                let v = resolve_value(value, param)?;
                match out.params.entry(param.id).or_insert_with(|| ParamFilter::Values(Vec::new())) {
                    ParamFilter::Values(vs) if !vs.contains(&v) => vs.push(v),
                    _ => {}
                }
            }
            FilterKind::Slider => {
                let (from, to) = match value.split_once("..") {
                    Some((f, t)) => (f.trim(), t.trim()),
                    None => (value.trim(), value.trim()),
                };
                let bound = |s: &str| (!s.is_empty()).then(|| resolve_value(s, param)).transpose();
                let (from, to) = (bound(from)?, bound(to)?);
                if from.is_none() && to.is_none() {
                    return Err(Error::InvalidQuery(format!("empty range for filter `{}`", param.name)));
                }
                if out.params.insert(param.id, ParamFilter::Range(from, to)).is_some() {
                    return Err(Error::InvalidQuery(format!("filter `{}` given twice", param.name)));
                }
            }
        }
    }
    for b in &query.branches {
        if !filters.branches.iter().any(|x| x.id == *b) {
            return Err(Error::InvalidQuery(format!(
                "unknown branch {b}; valid: {}",
                listed(&filters.branches, |x| format!("{} ({})", x.name, x.id))
            )));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_filters;

    fn filters() -> Filters {
        let body = std::fs::read_to_string(format!("{}/tests/fixtures/params_category_sk.json", env!("CARGO_MANIFEST_DIR")))
            .unwrap();
        parse_filters(&body).unwrap()
    }

    fn query() -> Query {
        Query {
            site: Site::Sk,
            text: None,
            category: Some(CategoryRef::plain(18842920)),
            price_min: None,
            price_max: None,
            sort: Sort::Recommended,
            stock: Stock::Any,
            branches: vec![],
            conditions: vec![],
            discounted: false,
            alza_plus: false,
            rated_4_plus: false,
            producers: vec![],
            params: vec![],
        }
    }

    fn rejected(q: &Query) -> String {
        match resolve(q, &filters()) {
            Err(Error::InvalidQuery(m)) => m,
            other => panic!("expected invalid_query, got {other:?}"),
        }
    }

    #[test]
    fn category_args() {
        assert_eq!(parse_category_arg("18842920", None).unwrap(), (CategoryRef::plain(18842920), Site::Sk));
        assert_eq!(
            parse_category_arg("1:action:17", Some(Site::Cz)).unwrap(),
            (CategoryRef { id: 1, kind: "ACTION".into(), kind_id: 17 }, Site::Cz)
        );
        assert_eq!(
            parse_category_arg("https://www.alza.hu/foo/18851639.htm", None).unwrap(),
            (CategoryRef::plain(18851639), Site::Hu)
        );
        assert!(parse_category_arg("https://www.alza.sk/pocitace-a-notebooky", None).is_err());
        assert!(parse_category_arg("https://www.alza.cz/foo/1.htm", Some(Site::Sk)).is_err());
        assert!(parse_category_arg("https://example.com/foo/1.htm", None).is_err());
        assert!(parse_category_arg("notebooky", None).is_err());
    }

    #[test]
    fn product_args() {
        assert_eq!(parse_product_arg("7927612", None).unwrap(), (7927612, Site::Sk));
        assert_eq!(parse_product_arg("7927612", Some(Site::De)).unwrap(), (7927612, Site::De));
        assert_eq!(
            parse_product_arg("https://www.alza.cz/hobby/la-proromance-6-burger-grill-holder-d7106480.htm", None).unwrap(),
            (7106480, Site::Cz)
        );
        assert!(matches!(parse_product_arg("https://www.alza.sk/foo", None), Err(Error::InvalidProduct(_))));
        assert!(matches!(parse_product_arg("https://www.bazos.sk/x-d1.htm", None), Err(Error::InvalidProduct(_))));
        assert!(matches!(
            parse_product_arg("https://www.alza.cz/x-d1.htm", Some(Site::Sk)),
            Err(Error::InvalidQuery(_))
        ));
    }

    #[test]
    fn validation() {
        let mut q = query();
        assert!(q.validate().is_ok());
        q.text = Some("thinkpad".into());
        assert!(q.validate().is_err());
        q.category = None;
        assert!(q.validate().is_ok());
        q.text = Some("   ".into());
        assert!(q.validate().is_err());
        let mut q = query();
        q.branches = vec![30];
        assert!(q.validate().is_err());
        q.stock = Stock::Alza;
        assert!(q.validate().is_ok());
        let mut q = query();
        (q.price_min, q.price_max) = (Some(500.0), Some(100.0));
        assert!(q.validate().is_err());
    }

    #[test]
    fn resolves_producers_and_params_by_name_or_id() {
        let mut q = query();
        q.producers = vec!["lenovo".into(), "1627".into(), "Lenovo".into()];
        q.params = vec![
            "Typ panela=OLED".into(),
            "102=33811".into(),
            "Veľkosť operačnej pamäte RAM=16 GB..".into(),
            "  Pomer strán = 16 : 9 ".into(),
        ];
        let r = resolve(&q, &filters()).unwrap();
        assert_eq!(r.producers, vec![1776, 1627]);
        assert_eq!(r.params[&102], ParamFilter::Values(vec![915.0, 33811.0]));
        assert_eq!(r.params[&70], ParamFilter::Range(Some(16384.0), None));
        assert_eq!(r.params[&81], ParamFilter::Values(vec![102.0]));
    }

    #[test]
    fn slider_accepts_raw_numbers_and_exact_values() {
        let mut q = query();
        q.params = vec!["70=8192..32768".into(), "68=8".into()];
        let r = resolve(&q, &filters()).unwrap();
        assert_eq!(r.params[&70], ParamFilter::Range(Some(8192.0), Some(32768.0)));
        assert_eq!(r.params[&68], ParamFilter::Range(Some(8.0), Some(8.0)));
    }

    #[test]
    fn rejects_unknown_or_malformed_filters() {
        let mut q = query();
        q.producers = vec!["Nokiaxx".into()];
        assert!(rejected(&q).contains("Apple (1627)"));
        let mut q = query();
        q.params = vec!["Typ panela=Plazma".into()];
        assert!(rejected(&q).contains("OLED (915)"));
        q.params = vec!["Typ panela=1..2".into()];
        assert!(rejected(&q).contains("checkbox"));
        q.params = vec!["Nonexistent=1".into()];
        assert!(rejected(&q).contains("unknown filter"));
        q.params = vec!["70=..".into()];
        assert!(rejected(&q).contains("empty range"));
        q.params = vec!["70=1..".into(), "70=..2".into()];
        assert!(rejected(&q).contains("given twice"));
        q.params = vec!["no-equals-sign".into()];
        assert!(rejected(&q).contains("KEY=VALUE"));
        let mut q = query();
        (q.stock, q.branches) = (Stock::Alza, vec![5]);
        assert!(rejected(&q).contains("unknown branch"));
    }

    #[test]
    fn request_body() {
        let mut q = query();
        (q.sort, q.stock, q.branches) = (Sort::PriceAsc, Stock::Alza, vec![30]);
        q.conditions = vec![Condition::OpenBox, Condition::Used];
        (q.discounted, q.rated_4_plus, q.price_max) = (true, true, Some(900.0));
        let mut r = Resolved { producers: vec![1776], ..Default::default() };
        r.params.insert(70, ParamFilter::Range(Some(16384.0), None));
        let b = q.body(&r, 3);
        assert_eq!(b["id"], 18842920);
        assert_eq!(b["type"], "CATEGORY");
        assert_eq!(b["orderBy"], 1);
        assert_eq!(b["page"], 3);
        assert_eq!(b["availabilityType"], 2);
        assert_eq!(b["selectedBranches"], json!([30]));
        assert_eq!(b["commodityWears"], json!([2, 1, 3]));
        assert_eq!(b["maxPrice"], 900.0);
        assert_eq!(b["minPrice"], Value::Null);
        assert_eq!(b["producers"], json!([1776]));
        assert_eq!(b["params"], json!([{"tId": 70, "v": null, "vFrom": 16384.0, "vTo": null}]));
        assert_eq!(b["useRatingThreshold"], true);
        assert_eq!(b["showOnlyActionCommodities"], true);
        assert_eq!(b["searchTerm"], "");

        let mut q = query();
        (q.category, q.text) = (None, Some(" iphone ".into()));
        let b = q.body(&Resolved::default(), 1);
        assert_eq!((b["id"].clone(), b["type"].clone(), b["searchTerm"].clone()), (json!(0), json!(""), json!("iphone")));
        assert_eq!(b["commodityWears"], Value::Null);
        assert_eq!(b["selectedBranches"], json!([]));
    }
}
