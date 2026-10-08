use actix_identity::Identity;
use actix_web::{get, web, Error, HttpResponse};
use sqlx::MySqlPool;
use tera::{Context, Tera};

#[get("/")]
async fn index(
    identity: Option<Identity>,
    db_pool: web::Data<MySqlPool>,
    tmpl: web::Data<Tera>,
) -> Result<HttpResponse, Error> {
    let mut context = Context::new();

    // Check if user has an active identity session
    if let Some(user_identity) = identity {
        if let Ok(user_id_str) = user_identity.id() {
            // Parse the stored user ID string to integer
            if let Ok(user_id) = user_id_str.parse::<i32>() {
                // Fetch the actual username from MySQL
                let user_query = sqlx::query!(
                    "SELECT username FROM users WHERE id = ?",
                    user_id
                )
                .fetch_optional(db_pool.get_ref())
                .await;

                if let Ok(Some(user)) = user_query {
                    // Inject username into Tera context (used by base.html)
                    context.insert("user", &user.username);
                    context.insert("user_id", &user_id);
                }
            }
        }
    }

    let rendered = tmpl.render("index.html", &context).map_err(|e| {
        eprintln!("Template rendering error: {}", e);
        actix_web::error::ErrorInternalServerError("Template error")
    })?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(rendered))
}
