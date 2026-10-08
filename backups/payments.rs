mod electrum;

use electrum::{get_address_balance}
use std::sync::Mutex;
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
    request: web::Json<PaymentRequest>,
    payment_addresses: web::Data<Mutex<Vec<PaymentAddress>>>,
    client: web::Data<Client>,
    identity: Option<Identity>,
) -> impl Responder {
    let (bet_id, outcome) = path.into_inner();

    // 2. Validate that outcome is strictly 1 or 2
    if outcome != 1 && outcome != 2 {
        return HttpResponse::BadRequest().body("Outcome must be 1 or 2");
    }
    
    let identity = match identity {
        Some(identity) => identity,
        None => {
            return HttpResponse::Unauthorized()
                .body("You must be logged in");
        }
    };

    let user_id = match identity.id() {
        Ok(id) => id,
        Err(_) => {
            return HttpResponse::Unauthorized()
                .body("Invalid identity");
        }
    };
    
    let reserved_address = {

        let mut addresses = match payment_addresses.lock() {
            Ok(guard) => guard,
            Err(_) => return HttpResponse::InternalServerError().body("Lock poisoned"),
        };

        let all_address_strings: Vec<String> = addresses
            .iter()
            .map(|a| a.address.clone())
            .collect();
        //TODO tuka mi trqbva funkciq koqto vzima address ne e v all_address_strings ili suzdava address ako nqma svobodni adresi

        let mut address = String::new();
        match get_all_addresses(&client).await {
            Ok(addresses) => {
                let mut available_address = false;
                for addr in addresses {
                    if !all_address_strings.contains(&addr.to_string()) {
                        address = addr;
                        available_address = true;
                        break;
                    }
                }
                if !available_address {
                    let new_address = create_new_address(&client);
                    address = new_address;
                }
            }
            Err(e) => {
                eprintln!("Failed to fetch addresses: {}", e);
            }
        }
        
        balance = get_address_balance(&client, address);
        let new_reservation = PaymentAddress {
            address,
            balance_before: balance,
            user_id,
            is_bet_bool: true,
            bet_id,
            outcome_id: outcome,
            error_logged: false,
        };
        
        addresses.push(new_reservation.clone());        

    };
    HttpResponse::SeeOther()
        .insert_header((header::LOCATION, "/user/orders"))
        .finish()
}
