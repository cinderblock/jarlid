//! Is the REST `stationId` always the same value as the tuner `stationToken`?
//!
//! This matters because `Engine::modes` holds a tuner token but the Modes endpoints are REST and
//! expect a `stationId`. They matched on the one station I first probed — which is a sample of
//! one. If they diverge for any station, mode switching would silently fail there, and silently
//! is the worst way for it to fail.
//!
//! Run: cargo run --example id-equivalence

use std::collections::HashMap;

#[tokio::main]
async fn main() {
    let Some((username, password)) = pandora::demo::credentials() else {
        eprintln!("Needs PANDORA_USERNAME / PANDORA_PASSWORD.");
        std::process::exit(2);
    };

    let mut client = pandora::Client::login(&username, &password).await.expect("login");

    let rest = client.stations().await.expect("rest stations");
    let tuner = client.station_list().await.expect("tuner stations");

    println!("REST stations:  {}", rest.len());
    println!("tuner stations: {}\n", tuner.len());

    // Duplicate names would silently collapse in a map and make a false "mismatch", so check
    // for them explicitly before drawing any conclusion from the comparison.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for station in &tuner {
        *counts.entry(station.station_name.as_str()).or_default() += 1;
    }
    let dupes: Vec<(&&str, &usize)> = counts.iter().filter(|(_, n)| **n > 1).collect();
    if dupes.is_empty() {
        println!("no duplicate station names in the tuner list");
    } else {
        println!("DUPLICATE tuner station names:");
        for (name, n) in &dupes {
            println!("  {name} x{n}");
            for s in tuner.iter().filter(|s| s.station_name == **name) {
                println!("      {} -> {}", s.station_name, s.station_token);
            }
        }
    }
    println!();

    let by_name: HashMap<&str, &str> = tuner
        .iter()
        .map(|s| (s.station_name.as_str(), s.station_token.as_str()))
        .collect();

    let mut matched = 0;
    let mut mismatched = Vec::new();
    let mut missing = Vec::new();

    for station in &rest {
        match by_name.get(station.name.as_str()) {
            Some(token) if *token == station.station_id => matched += 1,
            Some(token) => mismatched.push((station.name.clone(), station.station_id.clone(), token.to_string())),
            None => missing.push(station.name.clone()),
        }
    }

    println!("identical ids:   {matched}");
    println!("DIFFERENT ids:   {}", mismatched.len());
    println!("not in tuner:    {}", missing.len());

    for (name, rest_id, tuner_token) in mismatched.iter().take(10) {
        println!("  ⚠️  {name}\n      REST  {rest_id}\n      tuner {tuner_token}");
    }
    for name in missing.iter().take(10) {
        println!("  (only in REST) {name}");
    }

    println!();
    if mismatched.is_empty() && missing.is_empty() {
        println!("=> Safe: a tuner stationToken can be passed to the REST Modes endpoints.");
    } else {
        println!("=> NOT safe to treat them as interchangeable. Engine::modes must look up the");
        println!("   REST stationId by name (or the engine must track both ids per station).");
        std::process::exit(1);
    }
}
