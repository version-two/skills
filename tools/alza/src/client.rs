use std::collections::HashSet;
use std::time::Duration;

use futures::{StreamExt, TryStreamExt, stream};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::time::Instant;
use url::Url;

use crate::api::Envelope;
use crate::error::Error;
use crate::model::{
    Category, CategoryRef, Filters, Lang, Listing, Page, Product, ReviewStats, ReviewsResult, SearchResult, Site,
};
use crate::parse::{self, CategoryPage};
use crate::query::{self, PAGE_SIZE, Query, Resolved};

const APP_VERSION: &str = "2026.9.0;450;0;cz.alza.eshop";
const DEVICE: &str = "samsung/SM-S918B;14";
const MAX_ATTEMPTS: u32 = 3;

pub struct Client {
    http: reqwest::Client,
    lang: Option<Lang>,
    min_interval: Duration,
    last_request: Mutex<Option<Instant>>,
}

struct Fetched {
    status: StatusCode,
    body: String,
}

impl Client {
    pub fn new(timeout: Duration, min_interval: Duration, lang: Option<Lang>) -> Self {
        let http = reqwest::Client::builder().timeout(timeout).build().expect("reqwest client builds with static config");
        Self { http, lang, min_interval, last_request: Mutex::new(None) }
    }

    fn lang(&self, site: Site) -> Lang {
        self.lang.unwrap_or(site.default_lang())
    }

    /// Cloudflare only lets requests through that carry the app's User-Agent layout:
    /// `okhttp/<ver>;<maker>/<model>;<android>;<locale>;<app version>;<build>;<project>;<package>`.
    fn user_agent(&self, site: Site) -> String {
        format!("okhttp/5.2.1;{DEVICE};{};{APP_VERSION}", self.lang(site).tag().replace('-', "_"))
    }

    async fn throttle(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(t) = *last {
            let next = t + self.min_interval;
            if next > Instant::now() {
                tokio::time::sleep_until(next).await;
            }
        }
        *last = Some(Instant::now());
    }

    /// Retries transport errors, 429 and 5xx; any other status is returned for the caller to judge.
    async fn send(&self, site: Site, url: &Url, body: Option<&Value>) -> Result<Fetched, Error> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            self.throttle().await;
            let mut req = match body {
                Some(b) => self
                    .http
                    .post(url.clone())
                    .header("Content-Type", "application/json")
                    .body(serde_json::to_vec(b).expect("request body serializes")),
                None => self.http.get(url.clone()),
            };
            req = req
                .header("User-Agent", self.user_agent(site))
                .header("Accept", "application/json")
                .header("Accept-Language", self.lang(site).tag());
            let result = req.send().await;
            let retryable = match &result {
                Ok(r) => r.status() == StatusCode::TOO_MANY_REQUESTS || r.status().is_server_error(),
                Err(e) => e.is_timeout() || e.is_connect() || e.is_request(),
            };
            if retryable && attempt < MAX_ATTEMPTS {
                tokio::time::sleep(Duration::from_millis(1000 * 2u64.pow(attempt - 1))).await;
                continue;
            }
            let resp = result.map_err(|source| Error::Transport { url: url.to_string(), source })?;
            let status = resp.status();
            let text = resp.text().await.map_err(|source| Error::Transport { url: url.to_string(), source })?;
            return Ok(Fetched { status, body: text });
        }
    }

    async fn fetch_ok(&self, site: Site, url: &Url, body: Option<&Value>) -> Result<String, Error> {
        let f = self.send(site, url, body).await?;
        if !f.status.is_success() {
            return Err(Error::Http { status: f.status.as_u16(), url: url.to_string() });
        }
        Ok(f.body)
    }

    /// Fetches a legacy RestService endpoint, which reports failures in-band as `{"err": n, "msg": ...}` with HTTP 200.
    async fn fetch_rest(&self, site: Site, url: &Url, body: Option<&Value>) -> Result<(String, Envelope), Error> {
        let text = self.fetch_ok(site, url, body).await?;
        let env: Envelope = parse::json(&text).map_err(|reason| Error::Parse { url: url.to_string(), reason })?;
        Ok((text, env))
    }

    fn api_error(env: Envelope, url: &Url) -> Error {
        Error::Api { code: env.err, message: env.msg.unwrap_or_default(), url: url.to_string() }
    }

    fn rest_url(site: Site, path: &str, query: &[(&str, String)]) -> Url {
        let mut url = Url::parse(&format!("https://{}/services/restservice.svc/{path}", site.host())).unwrap();
        {
            let mut q = url.query_pairs_mut();
            for (k, v) in query {
                q.append_pair(k, v);
            }
            q.append_pair("country", site.country());
        }
        url
    }

    pub async fn category(&self, site: Site, category: &CategoryRef) -> Result<CategoryPage, Error> {
        let url = Self::rest_url(
            site,
            &format!("v1/category/{}", category.id),
            &[("t", category.kind.clone()), ("p", category.kind_id.to_string())],
        );
        let (text, env) = self.fetch_rest(site, &url, None).await?;
        match env.err {
            0 => parse::parse_category(&text, category).map_err(|reason| Error::Parse { url: url.to_string(), reason }),
            1 => Err(Error::InvalidQuery(format!(
                "category {}: {}",
                category.to_arg(),
                env.msg.unwrap_or_default()
            ))),
            _ => Err(Self::api_error(env, &url)),
        }
    }

    pub async fn categories(&self, site: Site, category: &CategoryRef) -> Result<Category, Error> {
        Ok(self.category(site, category).await?.category)
    }

    pub async fn filters(&self, site: Site, category: Option<&CategoryRef>, text: Option<&str>) -> Result<Filters, Error> {
        let root = CategoryRef::plain(0);
        let c = category.unwrap_or(&root);
        let url = Self::rest_url(
            site,
            &format!("v3/params/{}", c.id),
            &[
                ("type", c.kind.clone()),
                ("typeId", c.kind_id.to_string()),
                ("search", text.unwrap_or("").trim().to_string()),
            ],
        );
        let (text, env) = self.fetch_rest(site, &url, None).await?;
        if env.err != 0 {
            return Err(Self::api_error(env, &url));
        }
        parse::parse_filters(&text).map_err(|reason| Error::Parse { url: url.to_string(), reason })
    }

    async fn page(&self, q: &Query, resolved: &Resolved, page: u64) -> Result<(Page, Option<String>), Error> {
        let body = q.body(resolved, page);
        let (url, body) = match &q.category {
            Some(c) => (
                Self::rest_url(q.site, "v2/products", &[("categoryId", c.id.to_string())]),
                json!({ "filterParameters": body }),
            ),
            None => (Self::rest_url(q.site, "v5/search", &[]), body),
        };
        let (text, env) = self.fetch_rest(q.site, &url, Some(&body)).await?;
        if env.err != 0 {
            return Err(Self::api_error(env, &url));
        }
        let parsed = match q.category {
            Some(_) => parse::parse_products(&text, q.site),
            None => parse::parse_search(&text, q.site),
        };
        parsed.map_err(|reason| Error::Parse { url: url.to_string(), reason })
    }

    fn check_sort(q: &Query, allowed: &[i64]) -> Result<(), Error> {
        if allowed.contains(&q.sort.code()) {
            return Ok(());
        }
        Err(Error::InvalidQuery(format!(
            "sort `{}` is not offered for this {}",
            serde_json::to_value(q.sort).unwrap().as_str().unwrap(),
            if q.category.is_some() { "category" } else { "search" }
        )))
    }

    /// `offset` counts listings in Alza's order; pages hold 25 listings.
    pub async fn search(&self, q: &Query, offset: u64, limit: u64) -> Result<SearchResult, Error> {
        q.validate()?;
        if let Some(c) = &q.category {
            let page = self.category(q.site, c).await?;
            if page.category.section {
                return Err(Error::InvalidQuery(format!(
                    "`{}` ({}) is a section without its own product list; pick a subcategory from `alza categories {}`",
                    page.category.name,
                    c.to_arg(),
                    c.to_arg()
                )));
            }
            Self::check_sort(q, &page.sorts)?;
        }
        let resolved = if q.needs_filters() {
            let filters = self.filters(q.site, q.category.as_ref(), q.text.as_deref()).await?;
            query::resolve(q, &filters)?
        } else {
            Resolved::default()
        };

        let mut page_no = offset / PAGE_SIZE + 1;
        let mut skip = (offset % PAGE_SIZE) as usize;
        let mut listings: Vec<Listing> = Vec::new();
        let mut seen = HashSet::new();
        let mut first: Option<(u64, Option<String>, Vec<_>)> = None;
        loop {
            let (page, share_url) = self.page(q, &resolved, page_no).await?;
            if first.is_none() {
                if q.category.is_none() {
                    Self::check_sort(q, &page.sorts)?;
                }
                first = Some((page.total, share_url, page.related_categories.clone()));
            }
            let got = page.listings.len();
            let remaining = limit.saturating_sub(listings.len() as u64) as usize;
            listings.extend(page.listings.into_iter().skip(skip).filter(|l| seen.insert(l.id)).take(remaining));
            skip = 0;
            if listings.len() as u64 >= limit || !page.has_next {
                break;
            }
            if got == 0 {
                return Err(Error::Parse {
                    url: format!("page {page_no}"),
                    reason: "API reported more results but returned an empty page".into(),
                });
            }
            page_no += 1;
        }
        let (total, web_url, related_categories) = first.expect("at least one page fetched");
        Ok(SearchResult {
            site: q.site,
            query: q.text.as_ref().map(|t| t.trim().to_string()),
            category: q.category.clone(),
            web_url,
            total,
            offset,
            returned: listings.len(),
            listings,
            related_categories,
            details: None,
        })
    }

    fn product_url(site: Site, id: u64) -> Url {
        Url::parse(&format!("https://{}/api/router/legacy/catalog/product/{id}?country={}", site.host(), site.country()))
            .unwrap()
    }

    async fn detail(&self, site: Site, id: u64) -> Result<parse::ParsedDetail, Error> {
        let url = Self::product_url(site, id);
        let (text, env) = self.fetch_rest(site, &url, None).await?;
        match env.err {
            0 => parse::parse_detail(&text, site).map_err(|reason| Error::Parse { url: url.to_string(), reason }),
            1 => Err(Error::ProductGone { message: env.msg.unwrap_or_default(), url: url.to_string() }),
            _ => Err(Self::api_error(env, &url)),
        }
    }

    async fn review_stats(&self, site: Site, href: &str) -> Result<(ReviewStats, Option<String>), Error> {
        let url = Url::parse(href).map_err(|e| Error::Parse { url: href.to_string(), reason: e.to_string() })?;
        let text = self.fetch_ok(site, &url, None).await?;
        parse::parse_review_stats(&text).map_err(|reason| Error::Parse { url: href.to_string(), reason })
    }

    pub async fn product(&self, site: Site, id: u64) -> Result<Product, Error> {
        let detail = self.detail(site, id).await?;
        let mut product = detail.product;
        if let Some(href) = detail.review_stats_url {
            product.reviews = Some(self.review_stats(site, &href).await?.0);
        }
        Ok(product)
    }

    pub async fn products(&self, items: Vec<(u64, Site)>, concurrency: usize) -> Result<Vec<Product>, Error> {
        stream::iter(items)
            .map(|(id, site)| async move { self.product(site, id).await })
            .buffered(concurrency.max(1))
            .try_collect()
            .await
    }

    pub async fn reviews(&self, site: Site, id: u64, offset: u64, limit: u64) -> Result<ReviewsResult, Error> {
        let detail = self.detail(site, id).await?;
        let stats_href = detail.review_stats_url.ok_or_else(|| Error::Parse {
            url: Self::product_url(site, id).to_string(),
            reason: "product detail has no review stats link".into(),
        })?;
        let href = self.review_stats(site, &stats_href).await?.1.ok_or_else(|| Error::Parse {
            url: stats_href.clone(),
            reason: "review stats have no link to the review list".into(),
        })?;
        let base = Url::parse(&href).map_err(|e| Error::Parse { url: href.clone(), reason: e.to_string() })?;
        let mut reviews = Vec::new();
        let mut total = None;
        loop {
            let at = offset + reviews.len() as u64;
            let want = limit - reviews.len() as u64;
            let mut url = base.clone();
            {
                let kept: Vec<(String, String)> = base
                    .query_pairs()
                    .filter(|(k, _)| k != "limit" && k != "offset")
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                let mut q = url.query_pairs_mut();
                q.clear();
                for (k, v) in kept {
                    q.append_pair(&k, &v);
                }
                q.append_pair("limit", &want.max(1).to_string());
                q.append_pair("offset", &at.to_string());
            }
            let text = self.fetch_ok(site, &url, None).await?;
            let page = parse::parse_reviews(&text).map_err(|reason| Error::Parse { url: url.to_string(), reason })?;
            total.get_or_insert(page.total);
            let got = page.reviews.len();
            reviews.extend(page.reviews.into_iter().take(want as usize));
            if reviews.len() as u64 >= limit || got == 0 || offset + reviews.len() as u64 >= page.total {
                break;
            }
        }
        Ok(ReviewsResult {
            product_id: id,
            site,
            total: total.expect("at least one review page fetched"),
            offset,
            returned: reviews.len(),
            reviews,
        })
    }
}
