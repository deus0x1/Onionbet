use sqlx::{MySqlPool, FromRow};

#[derive(Debug, FromRow)]
pub struct UserBet {
    pub id: u64,
    pub user_id: u64,
    pub matket_id: u64,
    pub chosen_outcome: u8,
    pub amount_sats: u64,
    pub payout_sats: u64,
    pub status: String,
    pub created_at: Option<chrono::NaiveDateTime>,
}

pub async fn get_all_bets_for_market(
    market_id: u64,
    pool: &MySqlPool,
) -> Result<Vec<UserBet>, sqlx::Error> {
    let bets = sqlx::query_as::<_, UserBet>(
        "SELECT 
            id, 
            user_id, 
            market_id, 
            chosen_outcome, 
            amount_sats, 
            COALESCE(payout_sats, 0) AS payout_sats, 
            status, 
            created_at 
         FROM user_bets 
         WHERE market_id = ?"
    )
    .bind(market_id)
    .fetch_all(pool)
    .await?;

    Ok(bets)
}

async fn calculate_payout(
    user_bet_sats: u64,
    total_winning_bets_sats: u64,
    total_payout_pool: u64,
) -> u64 {
    if total_winning_bets_sats == 0 {
        return 0;
    }

    let user_sats = user_bet_sats as u128;
    let total_winning = total_winning_bets_sats as u128;
    let pool = total_payout_pool as u128;

    let payout = (user_sats * pool) / total_winning;

    payout as u64
}

#[post("/set_winner_bool/{bet_id}/{winning_outcode}")]
pub async fn set_winner_bool(
    path: web::Path<(u64, u8)>,
    client: web::Data<Client>,
    identity: Option<Identity>,
    pool: web::Data<MySqlPool>,
) -> impl Responder {

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

    let is_staff = match sqlx::query_scalar::<_, bool>(
        "SELECT is_staff FROM users WHERE id = ?"
    )
    .bind(user_id)
    .fetch_optional(pool.get_ref())
    .await 
    {
        Ok(Some(status)) => status,
        Ok(None) => return HttpResponse::Unauthorized().body("User not found"),
        Err(e) => {
            eprintln!("Database error checking staff status: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if !is_staff {
        return HttpResponse::Forbidden().body("Access denied: Staff only");
    }

    let (bet_id, winning_outcome) = path.into_inner();

    let status: String = match sqlx::query_scalar::<_, String>(
        "SELECT status FROM markets_bool WHERE id = ?"
    )
    .bind(bet_id)
    .fetch_optional(pool.get_ref())
    .await
    {
        Ok(Some(status)) => status,
        Ok(None) => return HttpResponse::NotFound().body("Market not found"),
        Err(e) => {
            eprintln!("Database error fetching market status: {e}");
            return HttpResponse::InternalServerError().finish();
        }
    };

    if status == "OPEN" {
        println!("Market is still open, close it if you want to process.");
        return HttpResponse::BadRequest().body("Market still open.");
    }


    let bets = match get_all_bets_for_market(bet_id, pool).await {
        Ok(bets) => bets,
        Err(e) => {
            eprintln!("Failed to fetch bets for market {market_id}: {e}");
            return HttpResponse::InternalServerError().body("Failed to load market bets");
        }
    }

    let mut winners = 0;
    for bet in &bets {
        bet.chosen_outcome == winning_outcome {
            winners += 1;
        }
    }

    
}


#[post("/close_market/market_id")]
pub async fn close_market(
     path: web::Path<(u64)>,    
) -> impl Responder {
    let market_id = path.into_inner();

}


