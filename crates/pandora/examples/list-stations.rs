//! Exercise the real typed client end to end, read-only.
//!
//! Proves the high-level API works against a live account: tuner login → REST → typed `Station`
//! models, including pagination across the whole collection.
//!
//! Run: cargo run --example list-stations

#[tokio::main]
async fn main() {
    let Some((username, password)) = pandora::demo::credentials() else {
        eprintln!("Needs PANDORA_USERNAME / PANDORA_PASSWORD (see .env.example at the repo root).");
        std::process::exit(2);
    };

    println!("logging in (tuner API) …");
    let mut client = match pandora::Client::login(&username, &password).await {
        Ok(client) => client,
        Err(e) => {
            eprintln!("login failed: {e}");
            std::process::exit(1);
        }
    };

    println!("fetching the station collection (REST) …");
    let stations = match client.stations().await {
        Ok(stations) => stations,
        Err(e) => {
            eprintln!("failed: {e}");
            std::process::exit(1);
        }
    };

    println!("\n{} stations\n", stations.len());
    for station in stations.iter().take(10) {
        println!(
            "  {:<34} {:>4}px art  #{}  {}",
            station.name.chars().take(34).collect::<String>(),
            station.hero_art().map(|a| a.size).unwrap_or(0),
            if station.dominant_color.is_empty() {
                "------".into()
            } else {
                station.dominant_color.clone()
            },
            station.station_type,
        );
    }
    if stations.len() > 10 {
        println!("  … and {} more", stations.len() - 10);
    }

    // Sanity-check the parse: empty names or ids would mean the model has drifted from the API.
    let unnamed = stations.iter().filter(|s| s.name.is_empty()).count();
    let idless = stations.iter().filter(|s| s.station_id.is_empty()).count();
    let art_less = stations.iter().filter(|s| s.art.is_empty()).count();
    let coloured = stations.iter().filter(|s| !s.dominant_color.is_empty()).count();

    println!("\n--- model health ---");
    println!("  missing name:          {unnamed}");
    println!("  missing stationId:     {idless}");
    println!("  missing art:           {art_less}");
    println!("  have dominantColor:    {coloured}/{}", stations.len());

    if unnamed == 0 && idless == 0 {
        println!("\n=> Typed client works against the live account.");
    } else {
        println!("\n!! Model drift: some stations parsed with empty required fields.");
        std::process::exit(1);
    }
}
