use actix_identity::Identity;
use actix_session::Session;
use actix_web::{get, post, web, Error, HttpMessage, HttpRequest, HttpResponse, Responder};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use captcha::Captcha;
use serde::Deserialize;
use sqlx::MySqlPool;
use tera::{Context, Tera};

// 1. Updated Form Structs
#[derive(Deserialize)]
pub struct RegisterForm {
    pub username: String,
    pub password: String,
    pub captcha_answer: String, // Extracted from HTML form submit
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

use captcha::filters::{Noise, Wave};
// 2. Helper function to build a Base64 CAPTCHA
fn generate_captcha() -> (String, String) {
    let mut captcha = Captcha::new();
    
    // Add 5 random characters, set width/height, and apply distortion filters
    captcha
        .add_chars(5)
        .apply_filter(Noise::new(0.2))
        .apply_filter(Wave::new(2.0, 10.0))
        .view(220, 120);

    let answer = captcha.chars_as_string();
    let png_bytes = captcha.as_png().expect("Failed to render CAPTCHA PNG");
    let base64_str = STANDARD.encode(png_bytes);

    (answer, base64_str)
}

fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

fn verify_password(hash: &str, password: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
}

// -----------------------------------------------------------------------------
// GET /register: Generates CAPTCHA, stores string in Session, renders HTML template
// -----------------------------------------------------------------------------
#[get("/register")]
async fn register(
    session: Session,
    tmpl: web::Data<Tera>,
) -> Result<HttpResponse, Error> {
    let (captcha_text, base64_img) = generate_captcha();

    // Store expected answer in session
    if let Err(e) = session.insert("captcha_answer", captcha_text.to_lowercase()) {
        eprintln!("Failed to store CAPTCHA in session: {:?}", e);
        return Err(actix_web::error::ErrorInternalServerError("Session error"));
    }

    // Pass base64 image string to Tera template
    let mut context = Context::new();
    context.insert("captcha_img", &base64_img);

    let rendered = tmpl.render("register.html", &context).map_err(|e| {
        eprintln!("Template error: {}", e);
        actix_web::error::ErrorInternalServerError("Template error")
    })?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(rendered))
}

// -----------------------------------------------------------------------------
// POST /register_post: Validates CAPTCHA first, then inserts into Database
// -----------------------------------------------------------------------------
#[post("/register_post")]
async fn register_post(
    session: Session,
    db_pool: web::Data<MySqlPool>,
    form: web::Form<RegisterForm>,
) -> impl Responder {
    // A. Retrieve & immediately consume the CAPTCHA answer (prevents replay attacks)
    let stored_captcha: Option<String> = session.get("captcha_answer").unwrap_or(None);
    let _ = session.remove("captcha_answer");

    let user_answer = form.captcha_answer.trim().to_lowercase();

    match stored_captcha {
        Some(correct_answer) if !correct_answer.is_empty() && user_answer == correct_answer => {
            // CAPTCHA passed -> Check if username exists
            let existing = sqlx::query!(
                "SELECT id FROM users WHERE username = ?",
                form.username
            )
            .fetch_optional(db_pool.get_ref())
            .await;

            if let Ok(Some(_)) = existing {
                return HttpResponse::BadRequest().body("Username already taken");
            }

            // Hash password and insert into MySQL
            let hashed_password = hash_password(&form.password);
            let result = sqlx::query!(
                "INSERT INTO users (username, password) VALUES (?, ?)",
                form.username,
                hashed_password
            )
            .execute(db_pool.get_ref())
            .await;

            match result {
                Ok(_) => HttpResponse::Found()
                    .insert_header(("Location", "/login"))
                    .finish(),

                Err(_) => HttpResponse::InternalServerError().body("Error registering user"),
            }
        }
        _ => HttpResponse::BadRequest().body("Invalid or expired CAPTCHA challenge."),
    }
}

// -----------------------------------------------------------------------------
// GET /login & POST /login_post (Actix Identity authentication)
// -----------------------------------------------------------------------------
#[get("/login")]
async fn login(tmpl: web::Data<Tera>) -> Result<HttpResponse, Error> {
    let context = Context::new();

    let rendered = tmpl.render("login.html", &context).map_err(|e| {
        eprintln!("Template render error: {}", e);
        actix_web::error::ErrorInternalServerError("Template render error")
    })?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(rendered))
}

#[post("/login_post")]
async fn login_post(
    req: HttpRequest,
    db_pool: web::Data<MySqlPool>,
    form: web::Form<LoginForm>,
) -> impl Responder {
    let user = sqlx::query!(
        "SELECT id, password FROM users WHERE username = ?",
        form.username
    )
    .fetch_one(db_pool.get_ref())
    .await;

    if let Ok(user) = user {
        if verify_password(&user.password, &form.password) {
            // Attach user ID to session using actix-identity
            if Identity::login(&req.extensions(), user.id.to_string()).is_ok() {
                return HttpResponse::Found()
                    .insert_header(("Location", "/"))
                    .finish();
            }
        }
    }

    HttpResponse::Unauthorized().body("Invalid username or password")
}

#[post("/logout")]
async fn logout(identity: Identity) -> impl Responder {
    identity.logout(); // Destroys the identity session
    HttpResponse::Found()
        .insert_header(("Location", "/"))
        .finish()
}
