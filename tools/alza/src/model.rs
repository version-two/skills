use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Site {
    Cz,
    Sk,
    Hu,
    At,
    De,
}

impl Site {
    pub fn tld(self) -> &'static str {
        match self {
            Site::Cz => "cz",
            Site::Sk => "sk",
            Site::Hu => "hu",
            Site::At => "at",
            Site::De => "de",
        }
    }

    pub fn host(self) -> String {
        format!("www.alza.{}", self.tld())
    }

    pub fn country(self) -> &'static str {
        match self {
            Site::Cz => "CZ",
            Site::Sk => "SK",
            Site::Hu => "HU",
            Site::At => "AT",
            Site::De => "DE",
        }
    }

    pub fn currency(self) -> &'static str {
        match self {
            Site::Cz => "CZK",
            Site::Hu => "HUF",
            Site::Sk | Site::At | Site::De => "EUR",
        }
    }

    /// The app offers no Austrian locale; it sends de-DE on alza.at.
    pub fn default_lang(self) -> Lang {
        match self {
            Site::Cz => Lang::Cs,
            Site::Sk => Lang::Sk,
            Site::Hu => Lang::Hu,
            Site::At | Site::De => Lang::De,
        }
    }

    pub fn from_host(host: &str) -> Option<Site> {
        let lower = host.to_ascii_lowercase();
        let tld = lower.strip_suffix('.').unwrap_or(&lower).rsplit('.').next()?;
        let domain_ok = lower == format!("alza.{tld}") || lower.ends_with(&format!(".alza.{tld}"));
        if !domain_ok {
            return None;
        }
        [Site::Cz, Site::Sk, Site::Hu, Site::At, Site::De].into_iter().find(|s| s.tld() == tld)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Cs,
    Sk,
    En,
    De,
    Hu,
}

impl Lang {
    pub fn tag(self) -> &'static str {
        match self {
            Lang::Cs => "cs-CZ",
            Lang::Sk => "sk-SK",
            Lang::En => "en-GB",
            Lang::De => "de-DE",
            Lang::Hu => "hu-HU",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Sort {
    Recommended,
    Bestselling,
    PriceAsc,
    PriceDesc,
    Rating,
    Newest,
}

impl Sort {
    pub fn code(self) -> i64 {
        match self {
            Sort::Recommended => 0,
            Sort::PriceAsc => 1,
            Sort::PriceDesc => 2,
            Sort::Newest => 5,
            Sort::Rating => 6,
            Sort::Bestselling => 7,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Stock {
    /// No availability filter
    Any,
    /// In stock at Alza or at a partner
    Anywhere,
    /// In stock at Alza (narrow to showrooms with --branch)
    Alza,
}

impl Stock {
    pub fn code(self) -> i64 {
        match self {
            Stock::Any => 0,
            Stock::Anywhere => 1,
            Stock::Alza => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Condition {
    New,
    /// Unsealed or tested, full warranty
    OpenBox,
    /// Like new or used
    Used,
}

impl Condition {
    pub fn wear_ids(self) -> &'static [i64] {
        match self {
            Condition::New => &[0],
            Condition::OpenBox => &[2],
            Condition::Used => &[1, 3],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CategoryRef {
    pub id: u64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "type_id")]
    pub kind_id: i64,
}

impl CategoryRef {
    pub fn plain(id: u64) -> Self {
        Self { id, kind: "CATEGORY".into(), kind_id: 0 }
    }

    /// Short form accepted by `--category`: `ID` for plain categories, `ID:TYPE:TYPE_ID` otherwise.
    pub fn to_arg(&self) -> String {
        if self.kind == "CATEGORY" && self.kind_id == 0 {
            self.id.to_string()
        } else {
            format!("{}:{}:{}", self.id, self.kind, self.kind_id)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Price {
    pub amount: f64,
    pub currency: &'static str,
    pub without_vat: f64,
    /// Price before the discount, when Alza shows one.
    pub original: Option<f64>,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Listing {
    pub id: u64,
    pub code: String,
    pub name: String,
    pub url: String,
    pub spec: Option<String>,
    pub image: Option<String>,
    /// Null when Alza shows no price, e.g. a product whose sale has ended.
    pub price: Option<Price>,
    /// Alza's text in place of a missing price, e.g. "Cena nebola stanovená".
    pub price_note: Option<String>,
    pub availability: Option<String>,
    pub can_buy: bool,
    pub rating: f64,
    pub rating_count: u64,
    pub promo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CategoryLink {
    pub name: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub category: Option<CategoryRef>,
    /// A landing page grouping subcategories: browsable with `categories`, but it has no product list of its own.
    pub section: bool,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Page {
    pub total: u64,
    pub has_next: bool,
    pub sorts: Vec<i64>,
    pub listings: Vec<Listing>,
    pub related_categories: Vec<CategoryLink>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub site: Site,
    pub query: Option<String>,
    pub category: Option<CategoryRef>,
    pub web_url: Option<String>,
    pub total: u64,
    pub offset: u64,
    pub returned: usize,
    pub listings: Vec<Listing>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related_categories: Vec<CategoryLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<Product>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Category {
    pub name: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub category: CategoryRef,
    pub section: bool,
    pub url: Option<String>,
    pub breadcrumbs: Vec<CategoryLink>,
    pub children: Vec<CategoryLink>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Parameter {
    pub name: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParameterGroup {
    pub name: String,
    pub parameters: Vec<Parameter>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Document {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RatingBucket {
    pub stars: u8,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewStats {
    /// Null when nobody has rated the product yet.
    pub average: Option<f64>,
    pub rating_count: u64,
    pub review_count: u64,
    pub recommendation_rate: Option<f64>,
    pub distribution: Vec<RatingBucket>,
    /// Null until at least 30 customers bought the product (`complaint_label` then says it is unknown).
    pub complaint_rate: Option<f64>,
    pub complaint_label: Option<String>,
    pub purchases: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Product {
    pub id: u64,
    pub site: Site,
    pub code: String,
    pub name: String,
    pub url: String,
    pub spec: Option<String>,
    pub price: Option<Price>,
    pub price_note: Option<String>,
    pub availability: Option<String>,
    pub availability_note: Option<String>,
    pub in_stock: bool,
    pub can_buy: bool,
    pub warranty: Option<String>,
    pub producer_id: Option<u64>,
    pub part_number: Option<String>,
    pub category: Option<CategoryLink>,
    pub breadcrumbs: Vec<CategoryLink>,
    pub rating: f64,
    pub reviews: Option<ReviewStats>,
    pub promo: Option<String>,
    pub images: Vec<String>,
    pub parameters: Vec<ParameterGroup>,
    pub documents: Vec<Document>,
    pub description_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Review {
    pub rating: f64,
    pub author: String,
    /// ISO 8601 UTC timestamp as Alza returns it.
    pub reviewed_at: String,
    /// The product variant the review was written for.
    pub variant: Option<String>,
    pub text: String,
    pub positives: Vec<String>,
    pub negatives: Vec<String>,
    pub verified_purchase: bool,
    pub translated: bool,
    pub likes: u64,
    pub images: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewPage {
    pub total: u64,
    pub reviews: Vec<Review>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewsResult {
    pub product_id: u64,
    pub site: Site,
    pub total: u64,
    pub offset: u64,
    pub returned: usize,
    pub reviews: Vec<Review>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FilterValue {
    pub value: f64,
    pub label: String,
    pub count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterKind {
    Checkbox,
    Slider,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FilterParam {
    pub id: i64,
    pub group: String,
    pub name: String,
    pub kind: FilterKind,
    pub values: Vec<FilterValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Producer {
    pub id: i64,
    pub name: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Branch {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Filters {
    pub price_min: Option<f64>,
    pub price_max: Option<f64>,
    pub producers: Vec<Producer>,
    pub branches: Vec<Branch>,
    pub params: Vec<FilterParam>,
}
