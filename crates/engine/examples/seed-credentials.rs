//! One-time migration: move credentials from the development `.env` into the Windows Credential
//! Manager, which is where the app reads them.
//!
//! Verifies the credentials against Pandora before saving, so a stale `.env` cannot poison the
//! credential store. Equivalent to typing them into the app's sign-in form.
//!
//! Run: cargo run --example seed-credentials

#[tokio::main]
async fn main() {
    let Some((username, password)) = pandora::demo::credentials() else {
        eprintln!("No PANDORA_USERNAME / PANDORA_PASSWORD found in .env or the environment.");
        std::process::exit(2);
    };

    println!("verifying {username} against Pandora …");
    match pandora::Client::login(&username, &password).await {
        Ok(_) => println!("  ✅ accepted"),
        Err(e) => {
            eprintln!("  ❌ rejected: {e}");
            eprintln!("\nNot saving. Fix the credentials first.");
            std::process::exit(1);
        }
    }

    engine::credentials::store(&username, &password).expect("store");
    println!("\nSaved to the Windows Credential Manager. The app will pick these up on launch.");
    println!("Remove them from .env if you like — the app no longer needs that file.");
}
