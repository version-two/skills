mod client;
mod error;
mod model;
mod parse;
mod query;

use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use client::Client;
use error::Error;
use model::{Ad, Listing, Site, Sort};
use query::Query;

/// Search and read bazos.sk / bazos.cz classified ads. Output is JSON by default; errors go to stderr as JSON.
#[derive(Parser)]
#[command(name = "bazos", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[command(flatten)]
    global: GlobalOpts,
}

#[derive(Args)]
struct GlobalOpts {
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
    /// One JSON object per line (listings, ads or categories)
    Jsonl,
    /// Human-readable table
    Table,
}

#[derive(Subcommand)]
enum Command {
    /// Search ads with every filter bazos supports
    Search(SearchArgs),
    /// Fetch and parse one or more ads by URL (as returned in search results)
    Ad {
        #[arg(required = true)]
        urls: Vec<String>,
        #[arg(long, default_value_t = 4)]
        concurrency: usize,
    },
    /// List top-level categories, or subcategories of one category
    Categories {
        /// Category slug (e.g. `mobil`, `auto`); omit to list top-level categories
        category: Option<String>,
        #[arg(long, short = 's', value_enum, default_value_t = Site::Sk)]
        site: Site,
    },
}

#[derive(Args)]
struct SearchArgs {
    /// Search text (hledat)
    query: Option<String>,
    #[arg(long, short = 's', value_enum, default_value_t = Site::Sk)]
    site: Site,
    /// Category slug (rubrika), e.g. `mobil`, `auto`, `reality`; see `bazos categories`
    #[arg(long, short = 'c')]
    category: Option<String>,
    /// Subcategory slug within --category, e.g. `apple`; see `bazos categories <category>`
    #[arg(long)]
    subcategory: Option<String>,
    /// Postal code or place name (hlokalita), e.g. `81101` or `Žilina`
    #[arg(long, short = 'l')]
    location: Option<String>,
    /// Radius around --location in km (humkreis)
    #[arg(long, short = 'r')]
    radius: Option<u32>,
    /// Minimum price in site currency (cenaod)
    #[arg(long)]
    price_min: Option<u64>,
    /// Maximum price in site currency (cenado)
    #[arg(long)]
    price_max: Option<u64>,
    #[arg(long, short = 'o', value_enum, default_value_t = Sort::Newest)]
    sort: Sort,
    /// Maximum number of listings to return (fetches as many 20-item pages as needed)
    #[arg(long, short = 'n', default_value_t = 20)]
    limit: u64,
    /// Number of listings to skip
    #[arg(long, default_value_t = 0)]
    offset: u64,
    /// Drop paid TOP-promoted listings (bazos injects them regardless of query relevance)
    #[arg(long)]
    no_top: bool,
    /// Also fetch every listing's detail page (full description, images, seller, coordinates)
    #[arg(long)]
    details: bool,
    /// Parallel detail-page fetches when --details is set
    #[arg(long, default_value_t = 4)]
    concurrency: usize,
}

#[derive(Serialize)]
struct ErrorOut<'a> {
    error: &'a str,
    message: String,
}

fn emit<T: Serialize>(format: Format, doc: &T, items: &[impl Serialize]) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    match format {
        Format::Json => writeln!(out, "{}", serde_json::to_string_pretty(doc)?)?,
        Format::Jsonl => {
            for item in items {
                writeln!(out, "{}", serde_json::to_string(item)?)?;
            }
        }
        Format::Table => unreachable!("table output is rendered per command"),
    }
    Ok(())
}

fn price_cell(p: &model::Price) -> String {
    match (p.amount, &p.currency) {
        (Some(a), Some(c)) => format!("{a} {c}"),
        _ => p.raw.clone(),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

fn listing_table(total: u64, listings: &[Listing]) {
    println!("{} of {total} results", listings.len());
    for l in listings {
        println!(
            "{:<10} {:<12} {:>12} {:<18} {:>6} {}{}",
            l.date.as_deref().unwrap_or("-"),
            l.id,
            price_cell(&l.price),
            truncate(l.location.as_deref().unwrap_or("-"), 18),
            l.views.map(|v| v.to_string()).unwrap_or_default(),
            if l.top { "[TOP] " } else { "" },
            truncate(&l.title, 70),
        );
        println!("{:<10} {}", "", l.url);
    }
}

fn ad_table(ads: &[Ad]) {
    for a in ads {
        println!("{} ({})", a.title, a.id);
        println!("  url:      {}", a.url);
        println!("  price:    {}", a.price.as_ref().map(price_cell).unwrap_or_default());
        println!("  date:     {}", a.date.as_deref().unwrap_or("-"));
        println!("  seller:   {}", a.seller_name.as_deref().unwrap_or("-"));
        println!(
            "  location: {} {}",
            a.postal_code.as_deref().unwrap_or(""),
            a.location.as_deref().unwrap_or("")
        );
        println!("  views:    {}", a.views.map(|v| v.to_string()).unwrap_or_default());
        println!("  images:   {}", a.images.len());
        println!("\n{}\n", a.description);
    }
}

async fn run(cli: Cli) -> Result<(), Error> {
    let g = &cli.global;
    let client = Client::new(Duration::from_secs(g.timeout), Duration::from_millis(g.delay_ms));
    let format = g.format;
    let io = |r: anyhow::Result<()>| r.expect("writing to stdout");

    match cli.command {
        Command::Search(a) => {
            let q = Query {
                site: a.site,
                text: a.query,
                category: a.category,
                subcategory: a.subcategory,
                location: a.location,
                radius_km: a.radius,
                price_min: a.price_min,
                price_max: a.price_max,
                sort: a.sort,
            };
            let mut result = client.search(&q, a.offset, a.limit, a.no_top).await?;
            if a.details {
                let urls = result.listings.iter().map(|l| l.url.clone()).collect();
                result.details = Some(client.ads(urls, a.concurrency).await?);
            }
            match format {
                Format::Table => match &result.details {
                    Some(d) => ad_table(d),
                    None => listing_table(result.total, &result.listings),
                },
                Format::Jsonl if result.details.is_some() => {
                    io(emit(format, &result, result.details.as_deref().unwrap()))
                }
                _ => io(emit(format, &result, &result.listings)),
            }
        }
        Command::Ad { urls, concurrency } => {
            let ads = client.ads(urls, concurrency).await?;
            match format {
                Format::Table => ad_table(&ads),
                _ if ads.len() == 1 => io(emit(format, &ads[0], &ads)),
                _ => io(emit(format, &ads, &ads)),
            }
        }
        Command::Categories { category, site } => {
            let cats = match &category {
                None => client.categories(site).await?,
                Some(c) => client.subcategories(site, c).await?,
            };
            match format {
                Format::Table => {
                    for c in &cats {
                        println!("{:<16} {:<28} {}", c.slug, c.name, c.url);
                    }
                }
                _ => io(emit(format, &cats, &cats)),
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
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
