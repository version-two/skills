use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Site {
    Sk,
    Cz,
}

impl Site {
    pub fn tld(self) -> &'static str {
        match self {
            Site::Sk => "sk",
            Site::Cz => "cz",
        }
    }

    pub fn from_host(host: &str) -> Option<Site> {
        let lower = host.to_ascii_lowercase();
        if lower == "bazos.sk" || lower.ends_with(".bazos.sk") {
            Some(Site::Sk)
        } else if lower == "bazos.cz" || lower.ends_with(".bazos.cz") {
            Some(Site::Cz)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Sort {
    Newest,
    PriceAsc,
    PriceDesc,
    MostViewed,
    LeastViewed,
}

impl Sort {
    pub fn param(self) -> &'static str {
        match self {
            Sort::Newest => "",
            Sort::PriceAsc => "1",
            Sort::PriceDesc => "2",
            Sort::MostViewed => "3",
            Sort::LeastViewed => "4",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceKind {
    Amount,
    Negotiable,
    Offer,
    Free,
    InText,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Price {
    pub raw: String,
    pub kind: PriceKind,
    pub amount: Option<i64>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Listing {
    pub id: u64,
    pub url: String,
    pub category: String,
    pub title: String,
    pub snippet: String,
    pub price: Price,
    pub location: Option<String>,
    pub postal_code: Option<String>,
    pub views: Option<u64>,
    pub date: Option<String>,
    pub top: bool,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Breadcrumb {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ad {
    pub id: u64,
    pub url: String,
    pub site: Site,
    pub title: String,
    pub description: String,
    pub price: Option<Price>,
    pub date: Option<String>,
    pub top: bool,
    pub seller_name: Option<String>,
    pub location: Option<String>,
    pub postal_code: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub views: Option<u64>,
    pub category: Option<Breadcrumb>,
    pub subcategory: Option<Breadcrumb>,
    pub images: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchPage {
    pub total: u64,
    pub listings: Vec<Listing>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub site: Site,
    pub first_page_url: String,
    pub total: u64,
    pub offset: u64,
    pub returned: usize,
    pub listings: Vec<Listing>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<Ad>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Category {
    pub category: String,
    pub slug: String,
    pub name: String,
    pub url: String,
}
