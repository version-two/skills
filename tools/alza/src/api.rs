use serde::Deserialize;
use serde::de::IgnoredAny;

#[derive(Debug, Deserialize)]
pub struct Envelope {
    pub err: i64,
    pub msg: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Link {
    pub href: String,
}

#[derive(Debug, Deserialize)]
pub struct SortOption {
    pub sort: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceInfoV2 {
    pub price_with_vat: Option<String>,
    pub price_without_vat_no_currency: Option<f64>,
    pub compare_price: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListItem {
    pub id: u64,
    pub code: String,
    pub name: String,
    pub url: String,
    pub spec: Option<String>,
    pub img: Option<String>,
    pub price_no_currency: Option<f64>,
    pub price_info_v2: PriceInfoV2,
    pub avail: Option<String>,
    #[serde(rename = "can_buy")]
    pub can_buy: bool,
    pub rating: f64,
    pub rating_count: u64,
    #[serde(rename = "action_name")]
    pub action_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CategoryItem {
    pub name: String,
    pub ltp: bool,
    pub url: Option<String>,
    #[serde(rename = "self")]
    pub link: Link,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreadcrumbCategory {
    pub name: String,
    pub meta: Option<Link>,
    pub client_action: Option<Link>,
}

#[derive(Debug, Deserialize)]
pub struct Breadcrumb {
    pub category: BreadcrumbCategory,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub total: u64,
    #[serde(rename = "has_next")]
    pub has_next: bool,
    pub sorts: Vec<SortOption>,
    #[serde(rename = "shareURL")]
    pub share_url: Option<String>,
    pub data1: Vec<CategoryItem>,
    pub data2: Vec<ListItem>,
}

#[derive(Debug, Deserialize)]
pub struct ProductsResponse {
    pub total: u64,
    #[serde(rename = "has_next")]
    pub has_next: bool,
    #[serde(rename = "shareURL")]
    pub share_url: Option<String>,
    pub data: Vec<ListItem>,
}

#[derive(Debug, Deserialize)]
pub struct CategoryCurrent {
    pub name: String,
    pub ltp: bool,
    pub url: Option<String>,
    pub sorts: Vec<SortOption>,
}

#[derive(Debug, Deserialize)]
pub struct CategoryResponse {
    #[serde(rename = "cat_name")]
    pub cat_name: String,
    pub ltp: bool,
    pub current: CategoryCurrent,
    pub breadcrumbs: Vec<Breadcrumb>,
    pub data: Vec<CategoryItem>,
}

#[derive(Debug, Deserialize)]
pub struct ParamValue {
    pub v: f64,
    pub desc: String,
    pub cnt: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterParam {
    pub t_id: i64,
    pub name: String,
    pub render_type: String,
    pub values: Vec<ParamValue>,
}

#[derive(Debug, Deserialize)]
pub struct FilterGroup {
    pub name: String,
    pub params: Vec<FilterParam>,
}

#[derive(Debug, Deserialize)]
pub struct FilterGroups {
    pub groups: Vec<FilterGroup>,
}

#[derive(Debug, Deserialize)]
pub struct ProducerValue {
    pub v: i64,
    pub desc: String,
    pub cnt: u64,
}

#[derive(Debug, Deserialize)]
pub struct BranchItem {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct PriceStep {
    pub k: f64,
}

#[derive(Debug, Deserialize)]
pub struct ParamsResponse {
    pub params: Vec<FilterGroups>,
    pub producers: Vec<ProducerValue>,
    pub branches: Vec<BranchItem>,
    pub prices: Vec<PriceStep>,
}

#[derive(Debug, Deserialize)]
pub struct DetailValue {
    pub desc: String,
}

#[derive(Debug, Deserialize)]
pub struct DetailParam {
    pub name: String,
    pub values: Vec<DetailValue>,
}

#[derive(Debug, Deserialize)]
pub struct DetailGroup {
    pub name: String,
    pub params: Vec<DetailParam>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    pub orig_url: String,
}

#[derive(Debug, Deserialize)]
pub struct DocumentLink {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    #[serde(flatten)]
    pub item: ListItem,
    #[serde(rename = "is_in_stock")]
    pub is_in_stock: bool,
    #[serde(rename = "avail_postfix")]
    pub avail_postfix: Option<String>,
    pub warranty: Option<String>,
    pub producer_id: Option<u64>,
    #[serde(rename = "catalog_number")]
    pub catalog_number: Option<String>,
    pub category_id: Option<u64>,
    pub category_name: Option<String>,
    pub breadcrumb: Option<Vec<Breadcrumb>>,
    pub imgs: Option<Vec<Image>>,
    pub parameter_groups: Option<Vec<DetailGroup>>,
    pub links: Option<Vec<DocumentLink>>,
    pub desc_page_url: Option<String>,
    pub review_stats: Option<Link>,
}

#[derive(Debug, Deserialize)]
pub struct DetailResponse {
    pub data: Detail,
}

#[derive(Debug, Deserialize)]
pub struct RatingCount {
    pub value: u8,
    pub count: u64,
}

#[derive(Debug, Deserialize)]
pub struct Complaint {
    pub rate: Option<f64>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewStats {
    pub rating_average: Option<f64>,
    pub rating_count: u64,
    pub review_count: u64,
    pub recommendation_rate: Option<f64>,
    pub ratings: Vec<RatingCount>,
    pub complaint: Option<Complaint>,
    pub purchase_count_formatted: Option<String>,
    pub commodity_reviews_action: Option<Link>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewImage {
    pub image_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItem {
    pub rating: f64,
    pub like_count: u64,
    pub description: String,
    pub positives: Vec<String>,
    pub negatives: Vec<String>,
    pub name: String,
    pub verified_purchase_tag: Option<IgnoredAny>,
    pub review_date: String,
    pub commodity_name: Option<String>,
    pub is_translated: bool,
    pub images: Vec<ReviewImage>,
}

#[derive(Debug, Deserialize)]
pub struct Paging {
    pub size: u64,
}

#[derive(Debug, Deserialize)]
pub struct ReviewsResponse {
    pub paging: Paging,
    pub value: Vec<ReviewItem>,
}
