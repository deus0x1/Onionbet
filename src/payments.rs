mod electrum;

use electrum::{get_address_balance}
use tokio::sync::Mutex;
use actix_web::{post, web, HttpResponse, Responder};
use serde::Deserialize;
use actix_identity::Identity;
use sqlx::{MySqlPool, FromRow};

pub async fn get_used_addresses(pool: &MySqlPool) -> Result<Vec<String>, sqlx::Error> {
    let addresses = sqlx::query_scalar("SELECT address FROM payment_addresses")
        .fetch_all(pool)
        .await?;

    Ok(addresses)
}

#[derive(Debug, FromRow)]
pub struct PaymentAddressRecord {
    pub id: u64,                     // bigint unsigned
    pub address: String,             // varchar(255)
    pub balance_before: u64,         // bigint unsigned
    pub user_id: u32,                // int unsigned
    pub is_bet_bool: bool,           // tinyint(1)
    pub bet_id: u64,                 // bigint unsigned
    pub outcome_id: u32,             // int unsigned
    pub error_logged: bool,          // tinyint(1)
    pub created_at: Option<chrono::NaiveDateTime>, // timestamp (nullable)
}

pub async fn get_all_payment_addresses(
    pool: &MySqlPool,
) -> Result<Vec<PaymentAddressRecord>, sqlx::Error> {
    let records = sqlx::query_as::<_, PaymentAddressRecord>(
        "SELECT id, address, balance_before, user_id, is_bet_bool, bet_id, outcome_id, error_logged, created_at FROM payment_addresses"
    )
    .fetch_all(pool)
    .await?;

    Ok(records)
}

pub async fn look_for_payments(
    client: &Client,
    pool: &MySqlPool,
) {
    loop {
        let records = match get_all_payment_addresses(pool).await {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("Failed to fetch payment addresses: {e}");
                return HttpResponse::InternalServerError().finish();
            }
        };

        for record in records {
            //TODO tuk proverqvam balansa dali e povishen i ako e processvam paymenta i triq ot DB-a recorda
            match get_address_balance(client, record.address).await {
                Ok(balance_float) => {
                    //convert to satoshi
                    let balance_int: u64 = (balance_float * 100_000_000.0).round() as u64;

                    if balance_int > record.balance_before {
                        match process_bet_payment(pool, record, balance_int).await {
                            Ok(_) => {
                                if let Err(err) = sqlx::query("DELETE FROM payment_addresses WHERE id = ?")
                                    .bind(record.id as i64)
                                    .execute(pool)
                                    .await
                                {
                                    eprintln!("Payment processed, but failed to delete address ID-{}: {}", record.id, err);
                                }
                            }
                            Err(err) => {
                                eprintln!("Failed to process payment with ID-{}: {}", record.id, err);
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!("Failed to fetch balance: {}", err);
                }
            }
        }

        sleep(Duration::from_secs(10)).await;
    }
}

async fn process_bet_payment(
    pool: &MySqlPool,
    item: &PaymentAddressRecord,
    amount_sats: u64,
) -> Result<(), sqlx::Error> {
    // 1. Begin transaction
    let mut tx = pool.begin().await?;

    // 2. Insert into user_bets
    // MySQL BIGINT requires i64, TINYINT requires i8
    sqlx::query(
        "INSERT INTO user_bets (user_id, market_id, chosen_outcome, amount_sats, status) 
         VALUES (?, ?, ?, ?, 'CONFIRMED')"
    )
    .bind(item.user_id as i64)
    .bind(item.bet_id as i64)
    .bind(item.outcome_id as i8)
    .bind(amount_sats as i64)
    .execute(&mut *tx)
    .await?;

    // 3. Increment corresponding outcome balance in markets_bool
    if item.outcome_id == 1 {
        sqlx::query(
            "UPDATE markets_bool SET outcome1_balance = outcome1_balance + ? WHERE id = ?"
        )
        .bind(amount_sats as i64)
        .bind(item.bet_id as i64)
        .execute(&mut *tx)
        .await?;
    } else if item.outcome_id == 2 {
        sqlx::query(
            "UPDATE markets_bool SET outcome2_balance = outcome2_balance + ? WHERE id = ?"
        )
        .bind(amount_sats as i64)
        .bind(item.bet_id as i64)
        .execute(&mut *tx)
        .await?;
    }

    // 4. Commit transaction
    tx.commit().await?;

    Ok(())
}


#[derive(Deserialize)]
pub struct PaymentRequest {
    pub is_bet_bool: bool,
}

#[post("/make_payment/{bet_id}/{outcome}")]
async fn make_payment_handler(
    path: web::Path<(u64, u32)>,
    client: web::Data<Client>,
    identity: Option<Identity>,
    pool: web::Data<MySqlPool>,
    lock: web::Data<Mutex<()>>,
) -> impl Responder {
    let (bet_id, outcome) = path.into_inner();

    if outcome != 1 && outcome != 2 {
        return HttpResponse::BadRequest().body("Outcome must be 1 or 2");
    }
    
    let identity = match identity {
        Some(identity) => identity,
        None => return HttpResponse::Unauthorized().body("You must be logged in"),
    };

    let user_id_str = match identity.id() {
        Ok(id) => id,
        Err(_) => return HttpResponse::Unauthorized().body("Invalid identity"),
    };

    let user_id: u32 = match user_id_str.parse() {
        Ok(id) => id,
        Err(_) => return HttpResponse::BadRequest().body("Invalid user ID format"),
    };

    let _guard = lock.lock().await;

    let used_addresses = match get_used_addresses(&pool).await {
        Ok(addresses) => addresses,
        Err(err) => {
            eprintln!("Failed to get addresses: {err}");
            return HttpResponse::InternalServerError().finish(); // Auto-unlocks here!
        }
    };

    let mut address = String::new();
    if let Ok(fetched_addresses) = get_all_addresses(&client).await {
        for addr in fetched_addresses {
            if !used_addresses.contains(&addr) {
                address = addr;
                break;
            }
        }
        if address.is_empty() {
            if let Ok(new_address) = create_new_address(&client).await {
                address = new_address;
            }
        }
    }
    if address.is_empty() {
        eprintln!("Error: Failed to fetch or generate a payment address.");
        return HttpResponse::InternalServerError().body("Could not assign a payment address"); // Auto-unlocks here!
    }

    let balance = match get_address_balance(&client, &address).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to fetch balance for address {address}: {e}");
            return HttpResponse::InternalServerError().body("Failed to check wallet balance"); // Auto-unlocks here!
        }
    };

    let insert_result = sqlx::query(
        "INSERT INTO payment_addresses (address, balance_before, user_id, is_bet_bool, bet_id, outcome_id) 
         VALUES (?, ?, ?, ?, ?, ?)"
    )
    .bind(&address)
    .bind(balance)
    .bind(user_id)
    .bind(true)
    .bind(bet_id)
    .bind(outcome)
    .execute(pool.get_ref())
    .await;

    if let Err(e) = insert_result {
        eprintln!("Failed to insert payment address record: {e}");
        return HttpResponse::InternalServerError().body("Failed to record payment address"); // Auto-unlocks here!
    }

    HttpResponse::SeeOther()
        .insert_header((header::LOCATION, "/user/orders"))
        .finish() 
}
