use std::collections::HashSet;
use std::time::Duration;

use futures::{StreamExt, TryStreamExt, stream};
use reqwest::StatusCode;
use tokio::sync::Mutex;
use tokio::time::Instant;
use url::Url;

use crate::error::Error;
use crate::model::{Ad, Category, Listing, SearchResult, Site};
use crate::parse;
use crate::query::{PAGE_SIZE, Query};

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";
const MAX_ATTEMPTS: u32 = 3;

pub struct Client {
    http: reqwest::Client,
    min_interval: Duration,
    last_request: Mutex<Option<Instant>>,
}

pub struct Fetched {
    pub status: StatusCode,
    pub url: Url,
    pub body: String,
}

impl Client {
    pub fn new(timeout: Duration, min_interval: Duration) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(timeout)
            .build()
            .expect("reqwest client builds with static config");
        Self { http, min_interval, last_request: Mutex::new(None) }
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

    /// Retries transport errors, 429 and 5xx; any other status is returned to the caller to judge.
    async fn get(&self, url: &Url) -> Result<Fetched, Error> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            self.throttle().await;
            let result = self.http.get(url.clone()).send().await;
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
            let final_url = resp.url().clone();
            let body = resp.text().await.map_err(|source| Error::Transport { url: url.to_string(), source })?;
            return Ok(Fetched { status, url: final_url, body });
        }
    }

    async fn get_ok(&self, url: &Url) -> Result<Fetched, Error> {
        let f = self.get(url).await?;
        if !f.status.is_success() {
            return Err(Error::Http { status: f.status.as_u16(), url: url.to_string() });
        }
        Ok(f)
    }

    /// `offset` counts raw site positions (TOP listings included), so it maps directly onto bazos pages.
    pub async fn search(&self, query: &Query, offset: u64, limit: u64, exclude_top: bool) -> Result<SearchResult, Error> {
        query.validate()?;
        if let (Some(category), Some(sub)) = (&query.category, &query.subcategory) {
            let subs = self.subcategories(query.site, category).await?;
            let sub = sub.trim_matches('/');
            let (own, foreign): (Vec<&Category>, Vec<&Category>) = subs.iter().partition(|s| &s.category == category);
            let matches = own.iter().any(|s| s.slug == sub || s.slug.starts_with(&format!("{sub}/")));
            if !matches {
                if let Some(f) = foreign.iter().find(|s| s.slug == sub) {
                    return Err(Error::InvalidQuery(format!(
                        "subcategory `{sub}` belongs to category `{}`: use --category {} --subcategory {sub}",
                        f.category, f.category
                    )));
                }
                let known: Vec<&str> = own.iter().map(|s| s.slug.as_str()).collect();
                return Err(Error::InvalidQuery(format!(
                    "unknown subcategory `{sub}` in `{category}`; valid: {}",
                    known.join(", ")
                )));
            }
        }
        let mut page_offset = offset / PAGE_SIZE * PAGE_SIZE;
        let mut skip = (offset - page_offset) as usize;
        let mut listings: Vec<Listing> = Vec::new();
        let mut seen = HashSet::new();
        let mut total = None;

        while (listings.len() as u64) < limit {
            let url = query.page_url(page_offset);
            let f = self.get_ok(&url).await?;
            let page = parse::parse_search(&f.body, &f.url)
                .map_err(|reason| Error::Parse { url: url.to_string(), reason })?;
            total.get_or_insert(page.total);
            let got = page.listings.len();
            let remaining = (limit as usize) - listings.len();
            let fresh = page
                .listings
                .into_iter()
                .skip(skip)
                .filter(|l| !(exclude_top && l.top))
                .filter(|l| seen.insert(l.id))
                .take(remaining);
            listings.extend(fresh);
            skip = 0;
            page_offset += PAGE_SIZE;
            // Bazos serves short pages mid-results (e.g. 19 of 20), so only an empty page means the end.
            if got == 0 || page_offset >= page.total {
                break;
            }
        }

        Ok(SearchResult {
            site: query.site,
            first_page_url: query.page_url(offset / PAGE_SIZE * PAGE_SIZE).to_string(),
            total: total.unwrap_or(0),
            offset,
            returned: listings.len(),
            listings,
            details: None,
        })
    }

    pub async fn ad(&self, raw_url: &str) -> Result<Ad, Error> {
        let url = Url::parse(raw_url).map_err(|_| Error::InvalidAdUrl(raw_url.to_string()))?;
        let site = url
            .host_str()
            .and_then(Site::from_host)
            .filter(|_| url.path().starts_with("/inzerat/"))
            .ok_or_else(|| Error::InvalidAdUrl(raw_url.to_string()))?;
        let f = self.get(&url).await?;
        match f.status {
            StatusCode::NOT_FOUND | StatusCode::GONE => {
                return Err(Error::AdGone { status: f.status.as_u16(), url: url.to_string() });
            }
            s if !s.is_success() => return Err(Error::Http { status: s.as_u16(), url: url.to_string() }),
            _ => {}
        }
        parse::parse_ad(&f.body, &url, site).map_err(|reason| Error::Parse { url: url.to_string(), reason })
    }

    pub async fn ads(&self, urls: Vec<String>, concurrency: usize) -> Result<Vec<Ad>, Error> {
        stream::iter(urls).map(|u| async move { self.ad(&u).await }).buffered(concurrency.max(1)).try_collect().await
    }

    pub async fn categories(&self, site: Site) -> Result<Vec<Category>, Error> {
        let url = Url::parse(&format!("https://www.bazos.{}/", site.tld())).unwrap();
        let f = self.get_ok(&url).await?;
        parse::parse_categories(&f.body, site).map_err(|reason| Error::Parse { url: url.to_string(), reason })
    }

    pub async fn subcategories(&self, site: Site, category: &str) -> Result<Vec<Category>, Error> {
        if category.is_empty() || !category.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(Error::InvalidQuery(format!("category must be a slug like `mobil`, got `{category}`")));
        }
        let url = Url::parse(&format!("https://{category}.bazos.{}/", site.tld())).unwrap();
        let f = self.get_ok(&url).await?;
        parse::parse_subcategories(&f.body, &f.url).map_err(|reason| Error::Parse { url: url.to_string(), reason })
    }
}
