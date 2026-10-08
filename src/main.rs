mod authentication;
mod electrum;
mod api;
mod payments;

use payments::PaymentAddressRecord;
use actix_identity::IdentityMiddleware;
use actix_session::{storage::CookieSessionStore, SessionMiddleware};
use actix_web::{cookie::Key, web, App, HttpServer};
use authentication::{login, login_post, logout, register, register_post};
use electrum::get_all_addresses;
use reqwest::Client;
use sqlx::MySqlPool;
use tera::Tera;
use crate::api::index;
use tokio::sync::Mutex;


#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();

    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set in .env file");

    let db_pool = MySqlPool::connect(&database_url)
        .await
        .expect("Failed to connect to MySQL database");

    let tera = Tera::new("templates/**/*").expect("Failed to load Tera templates");

    let client = Client::new();
    match get_all_addresses(&client).await {
        Ok(addrs) => println!("Loaded {} addresses from Electrum.", addrs.len()),
        Err(e) => eprintln!("Failed to load Electrum addresses: {:?}", e),
    }

    let secret_key = Key::generate();

    let client_data = web::Data::new(client);
    let db_data = web::Data::new(db_pool);
    let tera_data = web::Data::new(tera);
    let payment_lock = web::Data::new(Mutex::new(()));

    println!("Starting server on http://127.0.0.1:8080");

    HttpServer::new(move || {
        App::new()
            .app_data(client_data.clone()) 
            .app_data(payment_lock.clone())
            .app_data(db_data.clone())
            .app_data(tera_data.clone())
            .wrap(IdentityMiddleware::default())
            .wrap(
                SessionMiddleware::builder(
                    CookieSessionStore::default(),
                    secret_key.clone(),
                )
                .cookie_secure(false)
                .build(),
            )
            .service(register)
            .service(register_post)
            .service(login)
            .service(login_post)
            .service(logout)
            .service(index)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
