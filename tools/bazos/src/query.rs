use url::Url;

use crate::error::Error;
use crate::model::{Site, Sort};

pub const PAGE_SIZE: u64 = 20;

#[derive(Debug, Clone)]
pub struct Query {
    pub site: Site,
    pub text: Option<String>,
    pub category: Option<String>,
    pub subcategory: Option<String>,
    pub location: Option<String>,
    pub radius_km: Option<u32>,
    pub price_min: Option<u64>,
    pub price_max: Option<u64>,
    pub sort: Sort,
}

impl Query {
    pub fn validate(&self) -> Result<(), Error> {
        if self.subcategory.is_some() && self.category.is_none() {
            return Err(Error::InvalidQuery("--subcategory requires --category".into()));
        }
        let slug_ok = |s: &str, allow_slash: bool| {
            !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || (allow_slash && c == '/'))
        };
        if let Some(c) = &self.category
            && !slug_ok(c, false)
        {
            return Err(Error::InvalidQuery(format!("--category must be a slug like `mobil`, got `{c}`")));
        }
        if let Some(s) = &self.subcategory
            && !slug_ok(s.trim_matches('/'), true)
        {
            return Err(Error::InvalidQuery(format!(
                "--subcategory must be a slug like `apple` or `prodam/byt`, got `{s}`"
            )));
        }
        if let (Some(min), Some(max)) = (self.price_min, self.price_max)
            && min > max
        {
            return Err(Error::InvalidQuery(format!("--price-min {min} is greater than --price-max {max}")));
        }
        if self.radius_km.is_some() && self.location.is_none() {
            return Err(Error::InvalidQuery("--radius requires --location".into()));
        }
        Ok(())
    }

    /// `offset` must be a multiple of PAGE_SIZE; bazos paginates in fixed 20-item pages.
    pub fn page_url(&self, offset: u64) -> Url {
        debug_assert_eq!(offset % PAGE_SIZE, 0);
        let tld = self.site.tld();
        let mut url = match &self.category {
            None => Url::parse(&format!("https://www.bazos.{tld}/search.php")).unwrap(),
            Some(category) => {
                let mut path = String::from("/");
                if let Some(sub) = &self.subcategory {
                    path.push_str(sub.trim_matches('/'));
                    path.push('/');
                }
                if offset > 0 {
                    path.push_str(&format!("{offset}/"));
                }
                Url::parse(&format!("https://{category}.bazos.{tld}{path}")).unwrap()
            }
        };
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("hledat", self.text.as_deref().unwrap_or(""));
            if self.category.is_none() {
                pairs.append_pair("rubriky", "www");
            }
            pairs.append_pair("hlokalita", self.location.as_deref().unwrap_or(""));
            pairs.append_pair("humkreis", &self.radius_km.map(|r| r.to_string()).unwrap_or_default());
            pairs.append_pair("cenaod", &self.price_min.map(|p| p.to_string()).unwrap_or_default());
            pairs.append_pair("cenado", &self.price_max.map(|p| p.to_string()).unwrap_or_default());
            pairs.append_pair("order", self.sort.param());
            if self.category.is_none() && offset > 0 {
                pairs.append_pair("crz", &offset.to_string());
            }
        }
        url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Query {
        Query {
            site: Site::Sk,
            text: Some("iphone 13".into()),
            category: None,
            subcategory: None,
            location: None,
            radius_km: None,
            price_min: None,
            price_max: None,
            sort: Sort::Newest,
        }
    }

    #[test]
    fn global_search_uses_search_php_and_crz() {
        let url = base().page_url(40);
        assert_eq!(
            url.as_str(),
            "https://www.bazos.sk/search.php?hledat=iphone+13&rubriky=www&hlokalita=&humkreis=&cenaod=&cenado=&order=&crz=40"
        );
    }

    #[test]
    fn category_search_uses_subdomain_and_path_offset() {
        let q = Query {
            site: Site::Cz,
            category: Some("mobil".into()),
            subcategory: Some("apple".into()),
            location: Some("11000".into()),
            radius_km: Some(20),
            price_min: Some(1000),
            price_max: Some(20000),
            sort: Sort::PriceDesc,
            ..base()
        };
        assert_eq!(
            q.page_url(20).as_str(),
            "https://mobil.bazos.cz/apple/20/?hledat=iphone+13&hlokalita=11000&humkreis=20&cenaod=1000&cenado=20000&order=2"
        );
        assert_eq!(q.page_url(0).path(), "/apple/");
    }

    #[test]
    fn validation() {
        assert!(Query { subcategory: Some("apple".into()), ..base() }.validate().is_err());
        assert!(Query { category: Some("mo bil".into()), ..base() }.validate().is_err());
        assert!(Query { price_min: Some(5), price_max: Some(1), ..base() }.validate().is_err());
        assert!(Query { radius_km: Some(5), ..base() }.validate().is_err());
        assert!(base().validate().is_ok());
    }
}
