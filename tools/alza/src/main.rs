mod api;
mod client;
mod error;
mod model;
mod parse;
mod query;

use std::fmt::Write as _;
use std::io::{ErrorKind, Write as _};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use client::Client;
use error::Error;
use model::{
    Category, CategoryRef, Condition, FilterKind, Filters, Lang, Listing, Price, Product, ReviewsResult, Site, Sort,
    Stock,
};
use query::Query;

/// Search alza.cz / alza.sk / alza.hu / alza.at / alza.de through the Alza mobile app API.
/// Output is JSON by default; errors go to stderr as JSON.
#[derive(Parser)]
#[command(name = "alza", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[command(flatten)]
    global: GlobalOpts,
}

#[derive(Args)]
struct GlobalOpts {
    /// Alza site; default `sk`, or the site of a URL argument
    #[arg(long, short = 's', value_enum, global = true)]
    site: Option<Site>,
    /// Response language; default is the site's own language
    #[arg(long, value_enum, global = true)]
    lang: Option<Lang>,
    /// Output format
    #[arg(long, short = 'f', value_enum, default_value_t = Format::Json, global = true)]
    format: Format,
    /// Minimum delay between HTTP requests in milliseconds
    #[arg(long, default_value_t = 300, global = true)]
    delay_ms: u64,
    /// Per-request timeout in seconds
    #[arg(long, default_value_t = 30, global = true)]
    timeout: u64,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    /// Single pretty-printed JSON document
    Json,
    /// One JSON object per line (listings, products, reviews, categories or filters)
    Jsonl,
    /// Human-readable table
    Table,
}

#[derive(Subcommand)]
enum Command {
    /// Search products by text, or list a category, with every filter the app offers
    Search(SearchArgs),
    /// Fetch full product details (parameters, images, review stats) by id or product URL
    Product {
        #[arg(required = true)]
        products: Vec<String>,
        /// Parallel product fetches
        #[arg(long, default_value_t = 4)]
        concurrency: usize,
    },
    /// Fetch customer reviews of one product
    Reviews {
        /// Product id or product URL
        product: String,
        /// Maximum number of reviews to return
        #[arg(long, short = 'n', default_value_t = 20)]
        limit: u64,
        /// Number of reviews to skip
        #[arg(long, default_value_t = 0)]
        offset: u64,
    },
    /// Browse the category tree; without REF lists the top level
    Categories {
        /// Category ref (`ID` or `ID:TYPE:TYPE_ID` as printed in `ref`) or category URL
        category: Option<String>,
    },
    /// List the filters (producers, parameters, branches, price range) for a search text or a category
    Filters {
        /// Search text
        query: Option<String>,
        /// Category ref or URL
        #[arg(long, short = 'c')]
        category: Option<String>,
    },
}

#[derive(Args)]
struct SearchArgs {
    /// Search text
    query: Option<String>,
    /// List this category instead of searching: ref (`ID` or `ID:TYPE:TYPE_ID`) or category URL
    #[arg(long, short = 'c')]
    category: Option<String>,
    /// Minimum price in the site currency
    #[arg(long)]
    price_min: Option<f64>,
    /// Maximum price in the site currency
    #[arg(long)]
    price_max: Option<f64>,
    #[arg(long, short = 'o', value_enum, default_value_t = Sort::Recommended)]
    sort: Sort,
    /// Availability filter
    #[arg(long, value_enum, default_value_t = Stock::Any)]
    stock: Stock,
    /// Only stock in this showroom (repeatable, needs --stock alza); ids from `alza filters`
    #[arg(long)]
    branch: Vec<i64>,
    /// Product condition (repeatable)
    #[arg(long, value_enum)]
    condition: Vec<Condition>,
    /// Only products in a promotion or discount
    #[arg(long)]
    discounted: bool,
    /// Only AlzaPlus+ products
    #[arg(long)]
    alza_plus: bool,
    /// Only products rated 4 stars or more
    #[arg(long)]
    rated_4_plus: bool,
    /// Producer name or id (repeatable); see `alza filters`
    #[arg(long)]
    producer: Vec<String>,
    /// Parameter filter `KEY=VALUE` (checkbox, repeat for several values) or `KEY=FROM..TO` (slider);
    /// KEY is the filter id or name, values are ids/numbers or labels; see `alza filters`
    #[arg(long)]
    param: Vec<String>,
    /// Maximum number of listings to return (fetches as many 25-item pages as needed)
    #[arg(long, short = 'n', default_value_t = 25)]
    limit: u64,
    /// Number of listings to skip
    #[arg(long, default_value_t = 0)]
    offset: u64,
    /// Also fetch every listing's full product details
    #[arg(long)]
    details: bool,
    /// Parallel product fetches when --details is set
    #[arg(long, default_value_t = 4)]
    concurrency: usize,
}

#[derive(Serialize)]
struct ErrorOut<'a> {
    error: &'a str,
    message: String,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum FilterLine<'a> {
    Producer(&'a model::Producer),
    Branch(&'a model::Branch),
    Param(&'a model::FilterParam),
}

fn render<T: Serialize>(format: Format, doc: &T, items: &[impl Serialize]) -> String {
    match format {
        Format::Json => serde_json::to_string_pretty(doc).expect("output serializes") + "\n",
        Format::Jsonl => items.iter().map(|i| serde_json::to_string(i).expect("output serializes") + "\n").collect(),
        Format::Table => unreachable!("table output is rendered per command"),
    }
}

fn money(amount: f64, currency: &str) -> String {
    if amount.fract() == 0.0 { format!("{amount:.0} {currency}") } else { format!("{amount:.2} {currency}") }
}

fn price_cell(p: &Option<Price>, note: &Option<String>) -> String {
    match p {
        None => note.clone().unwrap_or_else(|| "no price".into()),
        Some(Price { amount, currency, original: Some(o), .. }) => {
            format!("{} (was {})", money(*amount, currency), money(*o, currency))
        }
        Some(p) => money(p.amount, p.currency),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}

fn listing_table(total: u64, listings: &[Listing]) -> String {
    let mut o = format!("{} of {total} results\n", listings.len());
    for l in listings {
        writeln!(
            o,
            "{:<10} {:>24} {:>4.1}★{:<6} {:<24} {}",
            l.id,
            truncate(&price_cell(&l.price, &l.price_note), 24),
            l.rating,
            format!("({})", l.rating_count),
            truncate(l.availability.as_deref().unwrap_or("-"), 24),
            truncate(&l.name, 80),
        )
        .unwrap();
        writeln!(o, "{:<10} {}", "", l.url).unwrap();
    }
    o
}

fn product_table(products: &[Product]) -> String {
    let mut o = String::new();
    for p in products {
        writeln!(o, "{} ({}, {})", p.name, p.id, p.code).unwrap();
        writeln!(o, "  url:          {}", p.url).unwrap();
        writeln!(o, "  price:        {}", price_cell(&p.price, &p.price_note)).unwrap();
        let note = p.availability_note.as_deref().map(|n| format!(" – {n}")).unwrap_or_default();
        writeln!(o, "  availability: {}{note}", p.availability.as_deref().unwrap_or("-")).unwrap();
        writeln!(o, "  warranty:     {}", p.warranty.as_deref().unwrap_or("-")).unwrap();
        if let Some(r) = &p.reviews {
            let average = r.average.map(|a| format!("{a:.1}")).unwrap_or_else(|| "unrated".into());
            writeln!(o, "  rating:       {average} from {} ratings, {} reviews", r.rating_count, r.review_count).unwrap();
        }
        let crumbs: Vec<&str> = p.breadcrumbs.iter().map(|b| b.name.as_str()).collect();
        writeln!(o, "  category:     {}", crumbs.join(" > ")).unwrap();
        writeln!(o, "  images:       {}", p.images.len()).unwrap();
        if let Some(s) = &p.spec {
            writeln!(o, "\n  {s}").unwrap();
        }
        for g in &p.parameters {
            writeln!(o, "\n  {}", g.name).unwrap();
            for x in &g.parameters {
                writeln!(o, "    {:<40} {}", truncate(&x.name, 40), x.values.join(", ")).unwrap();
            }
        }
        o.push('\n');
    }
    o
}

fn reviews_table(result: &ReviewsResult) -> String {
    let mut o = format!("{} of {} reviews\n", result.returned, result.total);
    for r in &result.reviews {
        let verified = if r.verified_purchase { " (verified)" } else { "" };
        let date = r.reviewed_at.get(..10).unwrap_or(&r.reviewed_at);
        writeln!(o, "\n{:.1}★ {date} {}{verified}", r.rating, r.author).unwrap();
        if !r.text.is_empty() {
            writeln!(o, "  {}", r.text).unwrap();
        }
        for p in &r.positives {
            writeln!(o, "  + {p}").unwrap();
        }
        for n in &r.negatives {
            writeln!(o, "  - {n}").unwrap();
        }
    }
    o
}

fn category_table(cat: &Category) -> String {
    let mut o = format!("{} ({}){}\n", cat.name, cat.reference, if cat.section { " section" } else { "" });
    for c in &cat.children {
        let section = if c.section { "section" } else { "" };
        writeln!(o, "{:<24} {:<8} {}", c.reference.as_deref().unwrap_or("-"), section, c.name).unwrap();
    }
    o
}

fn filters_table(f: &Filters) -> String {
    let range = |v: Option<f64>| v.map(|x| x.to_string()).unwrap_or_else(|| "-".into());
    let mut o = format!("price: {} .. {}\n\nproducers:\n", range(f.price_min), range(f.price_max));
    for p in &f.producers {
        writeln!(o, "  {:<8} {:>6}  {}", p.id, p.count, p.name).unwrap();
    }
    if !f.branches.is_empty() {
        o.push_str("\nbranches:\n");
        for b in &f.branches {
            writeln!(o, "  {:<8} {}", b.id, b.name).unwrap();
        }
    }
    let mut group = "";
    for p in &f.params {
        if p.group != group {
            group = &p.group;
            writeln!(o, "\n{group}:").unwrap();
        }
        let kind = match p.kind {
            FilterKind::Checkbox => "checkbox",
            FilterKind::Slider => "slider",
        };
        let values: Vec<String> = p.values.iter().map(|v| format!("{}={}", v.label, v.value)).collect();
        writeln!(o, "  {:<8} {:<9} {:<36} {}", p.id, kind, truncate(&p.name, 36), truncate(&values.join(" | "), 120))
            .unwrap();
    }
    o
}

fn category_scope(raw: Option<&str>, site: Option<Site>) -> Result<(Option<CategoryRef>, Site), Error> {
    match raw {
        Some(raw) => {
            let (c, s) = query::parse_category_arg(raw, site)?;
            Ok((Some(c), s))
        }
        None => Ok((None, site.unwrap_or(Site::Sk))),
    }
}

async fn run(cli: Cli) -> Result<String, Error> {
    let g = &cli.global;
    let client = Client::new(Duration::from_secs(g.timeout), Duration::from_millis(g.delay_ms), g.lang);
    let format = g.format;

    let out = match cli.command {
        Command::Search(a) => {
            let (category, site) = category_scope(a.category.as_deref(), g.site)?;
            let q = Query {
                site,
                text: a.query,
                category,
                price_min: a.price_min,
                price_max: a.price_max,
                sort: a.sort,
                stock: a.stock,
                branches: a.branch,
                conditions: a.condition,
                discounted: a.discounted,
                alza_plus: a.alza_plus,
                rated_4_plus: a.rated_4_plus,
                producers: a.producer,
                params: a.param,
            };
            let mut result = client.search(&q, a.offset, a.limit).await?;
            if a.details {
                let items = result.listings.iter().map(|l| (l.id, site)).collect();
                result.details = Some(client.products(items, a.concurrency).await?);
            }
            match (format, &result.details) {
                (Format::Table, Some(d)) => product_table(d),
                (Format::Table, None) => listing_table(result.total, &result.listings),
                (_, Some(d)) => render(format, &result, d),
                (_, None) => render(format, &result, &result.listings),
            }
        }
        Command::Product { products, concurrency } => {
            let items =
                products.iter().map(|raw| query::parse_product_arg(raw, g.site)).collect::<Result<Vec<_>, _>>()?;
            let products = client.products(items, concurrency).await?;
            match format {
                Format::Table => product_table(&products),
                _ if products.len() == 1 => render(format, &products[0], &products),
                _ => render(format, &products, &products),
            }
        }
        Command::Reviews { product, limit, offset } => {
            let (id, site) = query::parse_product_arg(&product, g.site)?;
            let result = client.reviews(site, id, offset, limit).await?;
            match format {
                Format::Table => reviews_table(&result),
                _ => render(format, &result, &result.reviews),
            }
        }
        Command::Categories { category } => {
            let (c, site) = category_scope(category.as_deref(), g.site)?;
            let cat = client.categories(site, &c.unwrap_or(CategoryRef::plain(0))).await?;
            match format {
                Format::Table => category_table(&cat),
                _ => render(format, &cat, &cat.children),
            }
        }
        Command::Filters { query: text, category } => {
            let text = text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
            let (c, site) = category_scope(category.as_deref(), g.site)?;
            match (&text, &c) {
                (None, None) => return Err(Error::InvalidQuery("give a search text or --category".into())),
                (Some(_), Some(_)) => {
                    return Err(Error::InvalidQuery(
                        "search text and --category cannot be combined; the API has no category-scoped search".into(),
                    ));
                }
                _ => {}
            }
            let filters = client.filters(site, c.as_ref(), text.as_deref()).await?;
            match format {
                Format::Table => filters_table(&filters),
                _ => {
                    let lines: Vec<FilterLine> = filters
                        .producers
                        .iter()
                        .map(FilterLine::Producer)
                        .chain(filters.branches.iter().map(FilterLine::Branch))
                        .chain(filters.params.iter().map(FilterLine::Param))
                        .collect();
                    render(format, &filters, &lines)
                }
            }
        }
    };
    Ok(out)
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(out) => {
            let mut stdout = std::io::stdout().lock();
            match stdout.write_all(out.as_bytes()).and_then(|()| stdout.flush()) {
                Err(e) if e.kind() != ErrorKind::BrokenPipe => {
                    eprintln!("{}", serde_json::to_string(&ErrorOut { error: "io_error", message: e.to_string() }).unwrap());
                    ExitCode::FAILURE
                }
                _ => ExitCode::SUCCESS,
            }
        }
        Err(e) => {
            let mut message = e.to_string();
            let mut src = std::error::Error::source(&e);
            while let Some(s) = src {
                message.push_str(&format!(": {s}"));
                src = s.source();
            }
            let out = ErrorOut { error: e.code(), message };
            eprintln!("{}", serde_json::to_string(&out).unwrap());
            ExitCode::from(e.exit_code() as u8)
        }
    }
}
