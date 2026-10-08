mod electrum;

use electrum::{get_address_balance}
use tokio::sync::Mutex;
use actix_web::{post, web, HttpResponse, Responder};
use serde::Deserialize;
use actix_identity::Identity;


#[derive(Clone)]
pub struct PaymentAddress {
    pub address: String,
    pub balance_before: u64,
    pub user_id: u32,
    pub is_bet_bool: bool,
    pub bet_id: u64,
    pub outcome_id: u32,
    pub error_logged: bool,
}

//TODO: API ZA OPREDELQNE NA ADDRESS ZA PAYMENT I SUZDAVANE NA POYMENTADDRESS

pub async fn look_for_payments(
    client: &Client,
    addresses: &Mutex<Vec<PaymentAddress>>,
) {
    loop {
        // Step 1: Snapshot the list of addresses to check.
        // Lock drops immediately after cloning.
        let items_to_check: Vec<PaymentAddress> = {
            let guard = addresses.lock().unwrap();
            guard.clone() // Requires #[derive(Clone)] on PaymentAddress
        };

        // Track user_ids that have received payments
        let mut completed_user_ids = Vec::new();

        // Step 2: Iterate over the copied items without holding the lock
        for item in items_to_check {
            match get_address_balance(client, &item.address).await {
                Ok(new_balance) => {
                    let current_balance = (new_balance * 100_000_000.0) as u64;

                    // If a payment arrived
                    if current_balance > item.balance_before {
                        println!(
                            "Payment confirmed for user {} ({})! Amount: {}",
                            item.user_id, item.address, current_balance - item.balance_before
                        );
                        
                        let payment_satoshi = current_balance - item.balance_before;
                        println!(
                            "Payment confirmed for user {} ({})! Amount: {} sats",
                            item.user_id, item.address, payment_satoshi
                        );

                        let mut payment_processed = false;

                        if item.is_bet_bool {
                            // Validate outcome choice before executing DB query
                            if item.outcome_id == 1 || item.outcome_id == 2 {
                                match process_bet_payment(db_pool, &item, payment_satoshi).await {
                                    Ok(_) => {
                                        println!(
                                            "Successfully logged bet in DB for user {} on market {}",
                                            item.user_id, item.bet_id
                                        );
                                        payment_processed = true;
                                    }
                                    Err(err) => {
                                        eprintln!(
                                            "Failed to record bet in DB for user {}: {}",
                                            item.user_id, err
                                        );
                                        // Do not mark as completed so it can retry on the next sweep
                                    }
                                }
                            } else {
                                eprintln!(
                                    "Invalid outcome_id {} for user {}",
                                    item.outcome_id, item.user_id
                                );
                            }
                        } else {
                            // Non-bet payment logic (e.g. standard wallet deposit)
                            payment_processed = true;
                        }

                        if payment_processed {
                            completed_addresses.push(item.address.clone());
                        }
                    }
                }
                Err(err) => {
                    eprintln!("Failed to check balance for user {}: {}", item.user_id, err);
                }
            }
        }

        // Step 3: Remove all paid items from the shared Mutex vector
        if !completed_user_ids.is_empty() {
            let mut guard = addresses.lock().unwrap();
            // retain keeps elements that return `true` and drops elements that return `false`
            guard.retain(|item| !completed_user_ids.contains(&item.user_id));
        }

        // Wait before running the next polling sweep
        sleep(Duration::from_secs(10)).await;
    }
}

async fn process_bet_payment(
    pool: &MySqlPool,
    item: &PaymentAddress,
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
    _request: web::Json<PaymentRequest>,
    payment_addresses: web::Data<Mutex<Vec<PaymentAddress>>>,
    client: web::Data<Client>,
    identity: Option<Identity>,
) -> impl Responder {
    let (bet_id, outcome) = path.into_inner();

    if outcome != 1 && outcome != 2 {
        return HttpResponse::BadRequest().body("Outcome must be 1 or 2");
    }
    
    let identity = match identity {
        Some(identity) => identity,
        None => return HttpResponse::Unauthorized().body("You must be logged in"),
    };

    let user_id = match identity.id() {
        Ok(id) => id,
        Err(_) => return HttpResponse::Unauthorized().body("Invalid identity"),
    };

    // 1. LOCK FIRST
    let mut addresses = payment_addresses.lock().await;

    // 2. READ FROM LOCKED GUARD
    let all_address_strings: Vec<String> = addresses
        .iter()
        .map(|a| a.address.clone())
        .collect();

    // 3. FIND OR CREATE ADDRESS
    let mut address = String::new();
    if let Ok(fetched_addresses) = get_all_addresses(&client).await {
        for addr in fetched_addresses {
            if !all_address_strings.contains(&addr) {
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

    // 4. GUARD AGAINST EMPTY ADDRESSES
    if address.is_empty() {
        eprintln!("Error: Failed to fetch or generate a payment address.");
        return HttpResponse::InternalServerError().body("Could not assign a payment address");
    }
    
    // 5. FETCH BALANCE SAFELY
    let balance = match get_address_balance(&client, &address).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to fetch balance for address {address}: {e}");
            return HttpResponse::InternalServerError().body("Failed to check wallet balance");
        }
    };

    let new_reservation = PaymentAddress {
        address,
        balance_before: balance,
        user_id,
        is_bet_bool: true,
        bet_id,
        outcome_id: outcome,
        error_logged: false,
    };
    
    // 6. SAVE (No .clone() needed since new_reservation isn't used again)
    addresses.push(new_reservation);        

    HttpResponse::SeeOther()
        .insert_header((header::LOCATION, "/user/orders"))
        .finish()
}
