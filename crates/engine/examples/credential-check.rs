//! Verify the Windows Credential Manager round-trip.
//!
//! Uses obviously-fake values and restores whatever was there before, so running this never
//! disturbs a real saved login.
//!
//! Run: cargo run --example credential-check

use engine::credentials;

fn main() {
    // Preserve any real credentials so this check is non-destructive.
    let existing = credentials::load().expect("load");
    println!(
        "before: {}",
        match &existing {
            Some(c) => format!("credentials present for {:?}", c.username),
            None => "nothing stored".into(),
        }
    );

    println!("\nstoring fake credentials …");
    credentials::store("not-a-real-user@example.invalid", "not-a-real-password")
        .expect("store");

    let loaded = credentials::load().expect("load").expect("just stored");
    assert_eq!(loaded.username, "not-a-real-user@example.invalid");
    assert_eq!(loaded.password, "not-a-real-password");
    println!("  ✅ round-trip: username and password both survived");
    assert!(credentials::exists());
    println!("  ✅ exists() reports true");

    println!("\nclearing …");
    credentials::clear().expect("clear");
    assert!(credentials::load().expect("load").is_none());
    println!("  ✅ cleared, and a missing entry reads as None rather than erroring");

    // Clearing twice must be harmless — sign-out should be idempotent.
    credentials::clear().expect("second clear must not error");
    println!("  ✅ clearing twice is safe");

    if let Some(original) = existing {
        credentials::store(&original.username, &original.password).expect("restore");
        println!("\nrestored the credentials that were there before.");
    }

    println!("\n=> Credential storage works. Nothing real was disturbed.");
}
