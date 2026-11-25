//! The server of the catalog and a client for it.
//!
//! 1. `cargo run -p shop -- serve` serves the catalog on `http://127.0.0.1:3000/v1`. It prints one wide event per
//!    request.
//! 2. In another terminal the other commands call it with the generated client:
//!    `cargo run -p shop -- add-physical "Desk lamp" --sku LMP-0001 --fragile`,
//!    `cargo run -p shop -- list --limit 10` and `cargo run -p shop -- get 0`.

use clap::{Parser, Subcommand};
use shop::Catalog;
use shop::api::{self, operations as ops, types};

/// The catalog of a shop: its server and a client for it.
#[derive(Parser)]
struct Cli {
    /// The address the server listens on and the client connects to
    #[arg(long, global = true, default_value = "127.0.0.1:3000")]
    addr: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the catalog and print one wide event per request
    Serve,
    /// List the products
    List {
        /// At most this many products (100 at most)
        #[arg(long)]
        limit: Option<types::ListProductsLimit>,
    },
    /// Add a physical product
    AddPhysical {
        name: String,
        /// The stock keeping unit, such as LMP-0001
        #[arg(long)]
        sku: Option<String>,
        #[arg(long)]
        fragile: bool,
    },
    /// Add a digital product
    AddDigital {
        name: String,
        /// How many times it may be downloaded
        #[arg(long)]
        downloads: Option<i32>,
    },
    /// Show the product with this ID
    Get { id: i64 },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let transport = honig::ReqwestTransport::new(format!("http://{}/v1", cli.addr))?;
    let client = || api::Client::new(transport.clone()).with_api_key(shop::API_KEY);
    match cli.command {
        Command::Serve => serve(&cli.addr).await?,
        Command::List { limit } => {
            let params = ops::list_products::Params { limit, ..ops::list_products::Params::new() };
            match client().list_products(params).await? {
                ops::list_products::Response::Ok { body, .. } => println!("{}", serde_json::to_string_pretty(&body)?),
                ops::list_products::Response::Default { status, body } => {
                    return Err(answer(status.as_u16(), &body).into());
                }
            }
        }
        Command::AddPhysical { name, sku, fragile } => {
            let sku = sku.map(types::PhysicalSku::try_from).transpose()?;
            let product = types::Physical { sku, fragile: fragile.then_some(true), ..types::Physical::new(name) };
            add(&client(), types::Product::Physical(product)).await?;
        }
        Command::AddDigital { name, downloads } => {
            let product = types::Digital { downloads: downloads.into(), ..types::Digital::new(name) };
            add(&client(), types::Product::Digital(product)).await?;
        }
        Command::Get { id } => match client().get_product(ops::get_product::Params::new(id)).await? {
            ops::get_product::Response::Ok(product) => println!("{}", serde_json::to_string_pretty(&product)?),
            ops::get_product::Response::NotFound(error) => return Err(error.message.into()),
        },
    }
    Ok(())
}

// ANCHOR: server
async fn serve(addr: &str) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("serving the catalog on http://{addr}/v1");
    // One JSON line per request: the operation, the status, the duration, the body sizes and more.
    let app = api::Server::new(Catalog::default()).into_axum().layer(axum::middleware::map_response(log));
    axum::serve(listener, app).await
}

/// Prints the wide event that the server put in the extensions of the response.
async fn log(resp: axum::response::Response) -> axum::response::Response {
    if let Some(event) = resp.extensions().get::<api::RequestEvent>() {
        println!("{}", event.to_json());
    }
    resp
}
// ANCHOR_END: server

async fn add(
    client: &api::Client<impl api::Transport>,
    product: types::Product,
) -> Result<(), Box<dyn std::error::Error>> {
    match client.create_product(ops::create_product::Params::new(product)).await? {
        ops::create_product::Response::Created(product) => println!("{}", serde_json::to_string_pretty(&product)?),
        ops::create_product::Response::Default { status, body } => return Err(answer(status.as_u16(), &body).into()),
    }
    Ok(())
}

/// An error that the server answered with.
fn answer(status: u16, error: &types::Error) -> String {
    format!("{status}: {}", error.message)
}
