//! Verify Modes resolve on "Sandstorm Radio" — the awkward case on this account, because there
//! are **two** stations with that exact name.
//!
//! That duplicate is why the engine does not resolve the REST station id by name: a name lookup
//! here is genuinely ambiguous and could return the other station's modes. It matches on the id
//! instead, and refuses rather than guesses when a name is ambiguous.
//!
//! Run: cargo run --example modes-verify

#[tokio::main]
async fn main() {
    let Some((username, password)) = pandora::demo::credentials() else {
        eprintln!("Needs PANDORA_USERNAME / PANDORA_PASSWORD.");
        std::process::exit(2);
    };

    let (engine, _events) = engine::Engine::start(&username, &password).await.expect("start");

    let stations = engine.station_list().await.expect("stations");
    let duplicates: Vec<&pandora::TunerStation> = stations
        .iter()
        .filter(|s| s.station_name.contains("Sandstorm"))
        .collect();

    println!("stations named like \"Sandstorm\": {}", duplicates.len());
    for s in &duplicates {
        println!("  {} -> {}", s.station_name, s.station_token);
    }

    let Some((name, token)) = duplicates
        .first()
        .map(|s| (s.station_name.clone(), s.station_token.clone()))
    else {
        eprintln!("\nnone found; can't exercise the ambiguous case.");
        std::process::exit(1);
    };

    println!("\nselecting the first one …");
    if let Err(e) = engine.play_station(&name, &token).await {
        eprintln!("could not select the station: {e}");
        std::process::exit(1);
    }

    match engine.modes().await {
        Ok(modes) if modes.is_empty() => {
            println!("\n❌ no modes returned — id resolution failed for this station.");
            std::process::exit(1);
        }
        Ok(modes) => {
            println!("\n✅ {} modes resolved despite the duplicate name:", modes.len());
            for mode in &modes {
                println!("   [{}] {}", mode.mode_id, mode.label());
            }
        }
        Err(e) => {
            println!("\n❌ modes failed: {e}");
            std::process::exit(1);
        }
    }
}
