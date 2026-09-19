//! Direct network-player client for remote mode: discovers a MediaRenderer via
//! SSDP, then reads now-playing state straight from the device. For LinkPlay
//! devices (WiiM etc.) the native HTTP API is used — unlike plain DLNA
//! AVTransport it reports metadata for the device's OWN sources (WiiM-app
//! Pandora, presets, etc.), not just DLNA-pushed streams. AVTransport remains
//! the fallback for generic renderers.

use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tauri::Emitter;

const AVT: &str = "urn:schemas-upnp-org:service:AVTransport:1";

#[derive(Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteState {
    pub device: String,
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art: String,
    pub position: f64,
    pub duration: f64,
    /// 0-100; -1 when the device doesn't report volume (generic DLNA fallback).
    pub volume: f64,
    /// True when the device can start a Pandora station itself (a LinkPlay unit whose
    /// PlayQueue service was found). The Stations page shows a cast button per row only then.
    pub can_cast: bool,
}

#[derive(Clone)]
pub enum Target {
    /// LinkPlay/WiiM native HTTP API, `base` like "https://192.168.1.50".
    /// `pq_ctrl` is the proprietary PlayQueue UPnP control URL when the device
    /// advertises it — the only way to start a specific Pandora station.
    LinkPlay {
        base: String,
        name: String,
        pq_ctrl: Option<String>,
    },
    /// Generic DLNA AVTransport control endpoint.
    Upnp { ctrl_url: String, name: String },
}

impl Target {
    fn name(&self) -> &str {
        match self {
            Target::LinkPlay { name, .. } => name,
            Target::Upnp { name, .. } => name,
        }
    }
}

#[derive(Clone)]
pub struct RemoteCtl {
    pub target: Arc<tokio::sync::Mutex<Option<Target>>>,
}

impl RemoteCtl {
    pub fn new() -> Self {
        Self {
            target: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }
}

pub fn device_client() -> reqwest::Client {
    // WiiM firmware serves its API over HTTPS with a self-signed certificate.
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(4))
        .build()
        .unwrap_or_default()
}

// ---------- small parsing helpers ----------

/// Extract the text content of the first `<tag>` or `<ns:tag>` element.
fn tag_content(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    if let Some(i) = xml.find(&open) {
        let s = i + open.len();
        if let Some(e) = xml[s..].find(&format!("</{tag}>")) {
            return Some(xml[s..s + e].to_string());
        }
    }
    let needle = format!(":{tag}>");
    let mut from = 0;
    while let Some(i) = xml[from..].find(&needle) {
        let abs = from + i;
        if let Some(lt) = xml[..abs].rfind('<') {
            let name = &xml[lt + 1..abs + needle.len() - 1];
            if !name.starts_with('/') && !name.contains(' ') && !name.contains('<') {
                let s = abs + needle.len();
                if let Some(e) = xml[s..].find(&format!("</{name}>")) {
                    return Some(xml[s..s + e].to_string());
                }
            }
        }
        from = abs + needle.len();
    }
    None
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// "0:02:35" -> 155.0
fn hms_to_secs(s: &str) -> f64 {
    s.split(':')
        .filter_map(|p| p.parse::<f64>().ok())
        .fold(0.0, |acc, p| acc * 60.0 + p)
}

/// LinkPlay hex-encodes text fields ("426164" -> "Bad"). Non-hex passes through.
fn hex_decode(s: &str) -> String {
    let t = s.trim();
    if t.len() % 2 != 0 || t.is_empty() || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return t.to_string();
    }
    let bytes: Vec<u8> = (0..t.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&t[i..i + 2], 16).ok())
        .collect();
    String::from_utf8_lossy(&bytes).to_string()
}

// ---------- discovery ----------

/// Send an SSDP M-SEARCH out EVERY local IPv4 interface and collect
/// MediaRenderer description URLs. Multi-homed machines (Hyper-V switches,
/// multiple VLANs) send multicast on only the default interface otherwise, so
/// a renderer on another subnet is never discovered.
fn ssdp_search() -> Vec<String> {
    use std::collections::HashSet;
    use std::net::{IpAddr, Ipv4Addr};

    let mut ips: Vec<Ipv4Addr> = Vec::new();
    if let Ok(addrs) = if_addrs::get_if_addrs() {
        for a in addrs {
            if let IpAddr::V4(v4) = a.ip() {
                if !v4.is_loopback() && !v4.is_link_local() {
                    ips.push(v4);
                }
            }
        }
    }
    ips.sort();
    ips.dedup();

    if ips.is_empty() {
        return ssdp_search_iface(None);
    }
    // One socket per interface, in parallel — ~3s total, not 3s * N.
    let handles: Vec<_> = ips
        .into_iter()
        .map(|ip| std::thread::spawn(move || ssdp_search_iface(Some(ip))))
        .collect();
    let mut found: HashSet<String> = HashSet::new();
    for h in handles {
        if let Ok(urls) = h.join() {
            found.extend(urls);
        }
    }
    found.into_iter().collect()
}

fn ssdp_search_iface(iface: Option<std::net::Ipv4Addr>) -> Vec<String> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};

    let sock = match Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let _ = sock.set_reuse_address(true);
    let bind_ip = iface.unwrap_or(Ipv4Addr::UNSPECIFIED);
    let bind: SocketAddr = SocketAddrV4::new(bind_ip, 0).into();
    if sock.bind(&bind.into()).is_err() {
        return vec![];
    }
    if let Some(ip) = iface {
        let _ = sock.set_multicast_if_v4(&ip);
    }
    let _ = sock.set_read_timeout(Some(Duration::from_millis(400)));
    let udp: UdpSocket = sock.into();

    let msearch = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";
    for _ in 0..2 {
        let _ = udp.send_to(msearch.as_bytes(), ("239.255.255.250", 1900));
    }
    let mut found = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut buf = [0u8; 2048];
    while std::time::Instant::now() < deadline {
        if let Ok((n, _)) = udp.recv_from(&mut buf) {
            let resp = String::from_utf8_lossy(&buf[..n]);
            for line in resp.lines() {
                let lower = line.to_lowercase();
                if let Some(rest) = lower.strip_prefix("location:") {
                    let url = line[line.len() - rest.trim().len()..].trim().to_string();
                    if !found.contains(&url) {
                        found.push(url);
                    }
                }
            }
        }
    }
    found
}

/// Probe one SSDP location. Prefers the LinkPlay native API when the device
/// speaks it; otherwise returns a generic AVTransport target.
async fn probe_device(client: &reqwest::Client, location: &str) -> Option<Target> {
    let base_url = reqwest::Url::parse(location).ok()?;
    let host = base_url.host_str()?.to_string();

    let xml = client.get(location).send().await.ok()?.text().await.ok()?;
    let name = tag_content(&xml, "friendlyName").unwrap_or_else(|| "Network player".into());

    // LinkPlay probe: a valid getPlayerStatus JSON marks the native API.
    for scheme in ["https", "http"] {
        let api_base = format!("{scheme}://{host}");
        let url = format!("{api_base}/httpapi.asp?command=getPlayerStatus");
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(text) = resp.text().await {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if v.get("status").is_some() {
                        // The PlayQueue service lives in the UPnP description, on a different
                        // port than the HTTP API; resolve its control URL against the SSDP
                        // location so a station can be started later.
                        let pq_ctrl = xml
                            .split("<service>")
                            .find(|c| c.contains("urn:schemas-wiimu-com:service:PlayQueue"))
                            .and_then(|c| tag_content(c, "controlURL"))
                            .and_then(|u| base_url.join(&u).ok())
                            .map(|u| u.to_string());
                        eprintln!(
                            "[remote] {name}: LinkPlay API @ {api_base} (cast: {})",
                            pq_ctrl.is_some()
                        );
                        return Some(Target::LinkPlay {
                            base: api_base,
                            name,
                            pq_ctrl,
                        });
                    }
                }
            }
        }
    }

    // Generic DLNA fallback.
    let mut control = None;
    for chunk in xml.split("<service>") {
        if chunk.contains(AVT) {
            control = tag_content(chunk, "controlURL");
            break;
        }
    }
    let ctrl_url = base_url.join(&control?).ok()?.to_string();
    eprintln!("[remote] {name}: generic AVTransport @ {ctrl_url}");
    Some(Target::Upnp { ctrl_url, name })
}

// ---------- LinkPlay (WiiM) ----------

async fn linkplay_get(
    client: &reqwest::Client,
    base: &str,
    command: &str,
) -> Option<serde_json::Value> {
    let url = format!("{base}/httpapi.asp?command={command}");
    let text = client.get(&url).send().await.ok()?.text().await.ok()?;
    serde_json::from_str(&text).ok()
}

async fn poll_linkplay(
    client: &reqwest::Client,
    base: &str,
    name: &str,
    can_cast: bool,
) -> Option<RemoteState> {
    let status = linkplay_get(client, base, "getPlayerStatus").await?;
    let s = |k: &str| status.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let playing = matches!(s("status").as_str(), "play" | "loading" | "load");
    let position = s("curpos").parse::<f64>().unwrap_or(0.0) / 1000.0;
    let duration = s("totlen").parse::<f64>().unwrap_or(0.0) / 1000.0;
    let volume = s("vol").parse::<f64>().unwrap_or(-1.0);

    // getMetaInfo has clean text + album art (newer firmware); fall back to the
    // hex-encoded getPlayerStatus fields.
    let mut title = hex_decode(&s("Title"));
    let mut artist = hex_decode(&s("Artist"));
    let mut album = hex_decode(&s("Album"));
    let mut art = String::new();
    if let Some(meta) = linkplay_get(client, base, "getMetaInfo").await {
        if let Some(md) = meta.get("metaData") {
            let m = |k: &str| md.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !m("title").is_empty() {
                title = m("title");
                artist = m("artist");
                album = m("album");
            }
            art = m("albumArtURI");
        }
    }
    // Some sources report placeholder "unknow(n)" strings.
    let clean = |x: String| {
        let l = x.to_lowercase();
        if l == "unknow" || l == "unknown" || l == "un_known" {
            String::new()
        } else {
            x
        }
    };

    Some(RemoteState {
        device: name.to_string(),
        playing,
        title: clean(title),
        artist: clean(artist),
        album: clean(album),
        art,
        position,
        duration,
        volume,
        can_cast,
    })
}

// ---------- generic AVTransport ----------

async fn soap(
    client: &reqwest::Client,
    ctrl_url: &str,
    action: &str,
    args: &str,
) -> Option<String> {
    let envelope = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{action} xmlns:u="{AVT}"><InstanceID>0</InstanceID>{args}</u:{action}></s:Body></s:Envelope>"#
    );
    let resp = client
        .post(ctrl_url)
        .header("SOAPAction", format!("\"{AVT}#{action}\""))
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .body(envelope)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.text().await.ok()
}

async fn poll_upnp(client: &reqwest::Client, ctrl_url: &str, name: &str) -> Option<RemoteState> {
    let transport = soap(client, ctrl_url, "GetTransportInfo", "").await?;
    let state = tag_content(&transport, "CurrentTransportState").unwrap_or_default();
    let playing = state == "PLAYING" || state == "TRANSITIONING";

    let pos = soap(client, ctrl_url, "GetPositionInfo", "").await?;
    let duration = hms_to_secs(&tag_content(&pos, "TrackDuration").unwrap_or_default());
    let position = hms_to_secs(&tag_content(&pos, "RelTime").unwrap_or_default());
    let didl = xml_unescape(&tag_content(&pos, "TrackMetaData").unwrap_or_default());

    Some(RemoteState {
        device: name.to_string(),
        playing,
        title: xml_unescape(&tag_content(&didl, "title").unwrap_or_default()),
        artist: xml_unescape(
            &tag_content(&didl, "artist")
                .or_else(|| tag_content(&didl, "creator"))
                .unwrap_or_default(),
        ),
        album: xml_unescape(&tag_content(&didl, "album").unwrap_or_default()),
        art: xml_unescape(&tag_content(&didl, "albumArtURI").unwrap_or_default()),
        position,
        duration,
        volume: -1.0,
        can_cast: false,
    })
}

// ---------- presets (LinkPlay only) ----------

#[derive(Serialize)]
pub struct Preset {
    pub number: u32,
    pub name: String,
    pub source: String,
    pub art: String,
}

/// LinkPlay JSON is stringly-typed; accept numbers as numbers or strings.
fn jnum(v: Option<&serde_json::Value>) -> u64 {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

/// List the presets configured on the device (WiiM Home app presets 1-12).
pub async fn presets(client: &reqwest::Client, ctl: &RemoteCtl) -> Result<Vec<Preset>, String> {
    let target = ctl.target.lock().await.clone().ok_or("no network player found")?;
    let Target::LinkPlay { base, .. } = target else {
        return Err("presets require a WiiM/LinkPlay device".into());
    };
    let v = linkplay_get(client, &base, "getPresetInfo")
        .await
        .ok_or("preset query failed")?;
    let list = v
        .get("preset_list")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let s = |p: &serde_json::Value, k: &str| {
        p.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    Ok(list
        .iter()
        .map(|p| Preset {
            number: jnum(p.get("number")) as u32,
            name: s(p, "name"),
            source: s(p, "source"),
            art: s(p, "picurl"),
        })
        .filter(|p| p.number > 0 && !p.name.is_empty())
        .collect())
}

// ---------- PlayQueue (starting a station on a WiiM) ----------
//
// A WiiM plays Pandora itself; Jarlid never streams to it. Starting a specific station is a
// proprietary UPnP call on the wiimu PlayQueue service: build a queue whose search URL is
// `wiimu_search://<token>` (the token is Jarlid's own tuner station token — verified equal to
// the id the device uses), create it, then play it. The device fetches the audio from Pandora.

const WIIMU_PQ: &str = "urn:schemas-wiimu-com:service:PlayQueue:1";

/// Escape text for inclusion in an XML element. `&` first, or it double-escapes the others.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// One SOAP call to the PlayQueue service. Unlike AVTransport, its actions take no
/// `InstanceID`; `inner` is the full argument XML.
async fn pq_soap(
    client: &reqwest::Client,
    ctrl_url: &str,
    action: &str,
    inner: &str,
) -> Option<String> {
    let envelope = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{action} xmlns:u="{WIIMU_PQ}">{inner}</u:{action}></s:Body></s:Envelope>"#
    );
    let resp = client
        .post(ctrl_url)
        .header("SOAPAction", format!("\"{WIIMU_PQ}#{action}\""))
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .body(envelope)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.text().await.ok()
}

/// The account's numeric Pandora user id, read from the device's own linked-service list.
/// The station queue copies it into `Login_username`, matching a device-made queue.
async fn pandora_user_id(client: &reqwest::Client, pq_ctrl: &str) -> Option<String> {
    let resp = pq_soap(client, pq_ctrl, "GetBasicUserInfo", "").await?;
    let result = xml_unescape(&tag_content(&resp, "Result")?);
    let v: serde_json::Value = serde_json::from_str(&result).ok()?;
    v.get("streamServices")?
        .as_array()?
        .iter()
        .find(|s| s.get("id").and_then(|x| x.as_str()) == Some("Pandora2"))
        .and_then(|s| s.get("userId").and_then(|x| x.as_str()))
        .map(|s| s.to_string())
}

/// The queue-context XML for one Pandora station, shaped like a device-made queue.
fn station_queue_context(name: &str, token: &str, user_id: &str) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\"?>\n<PlayList>\n",
            "<ListName>{name}</ListName>\n<ListInfo>\n",
            "<SourceName>Pandora2</SourceName>\n",
            "<SearchUrl>wiimu_search://{token}</SearchUrl>\n",
            "<Login_username>{user}</Login_username>\n",
            "<MarkSearch>0</MarkSearch>\n<TrackNumber>0</TrackNumber>\n",
            "<TotalNumber>0</TotalNumber>\n<Quality>0</Quality>\n",
            "<requestQuality>High</requestQuality>\n<UpdateTime>0</UpdateTime>\n",
            "<LastPlayIndex>1</LastPlayIndex>\n<UserId>0</UserId>\n",
            "<StationBackup>1</StationBackup>\n<ContentType>station</ContentType>\n",
            "<SwitchPageMode>0</SwitchPageMode>\n<CurrentPage>0</CurrentPage>\n",
            "<TotalPages>0</TotalPages>\n<searching>0</searching>\n",
            "<PressType>0</PressType>\n<Volume>0</Volume>\n</ListInfo>\n",
            "<Tracks></Tracks>\n</PlayList>"
        ),
        name = xml_escape(name),
        token = xml_escape(token),
        user = xml_escape(user_id),
    )
}

/// Start a Pandora station on the current network player. `token` is Jarlid's tuner station
/// token. The device then streams the station itself.
pub async fn play_station(
    client: &reqwest::Client,
    ctl: &RemoteCtl,
    name: &str,
    token: &str,
) -> Result<(), String> {
    let target = ctl.target.lock().await.clone().ok_or("no network player found")?;
    let Target::LinkPlay {
        pq_ctrl: Some(pq), ..
    } = target
    else {
        return Err("this device can't start a Pandora station directly".into());
    };
    // Empty is acceptable — the device uses its own logged-in session; the id only matches a
    // device-made queue. A failed lookup should not block casting.
    let user_id = pandora_user_id(client, &pq).await.unwrap_or_default();
    let ctx = station_queue_context(name, token, &user_id);
    let create = format!("<QueueContext>{}</QueueContext>", xml_escape(&ctx));
    pq_soap(client, &pq, "CreateQueue", &create)
        .await
        .ok_or("CreateQueue failed")?;
    let play = format!("<QueueName>{}</QueueName><Index>1</Index>", xml_escape(name));
    pq_soap(client, &pq, "PlayQueueWithIndex", &play)
        .await
        .ok_or("PlayQueueWithIndex failed")?;
    Ok(())
}

// ---------- public API ----------

/// Issue a transport command ("play" | "pause" | "skip") to the current device.
pub async fn command(client: &reqwest::Client, ctl: &RemoteCtl, cmd: &str) -> Result<(), String> {
    let target = ctl.target.lock().await.clone().ok_or("no network player found")?;
    match target {
        Target::LinkPlay { base, .. } => {
            let lp_cmd = match cmd {
                "play" => "setPlayerCmd:resume".to_string(),
                "pause" => "setPlayerCmd:pause".to_string(),
                "skip" => "setPlayerCmd:next".to_string(),
                "prev" => "setPlayerCmd:prev".to_string(),
                // "preset:N" fires device preset N like its hardware button
                c if c.starts_with("preset:") => {
                    let n: u32 = c[7..].parse().map_err(|_| "bad preset number")?;
                    format!("MCUKeyShortClick:{n}")
                }
                // "vol:N" sets device volume 0-100
                c if c.starts_with("vol:") => {
                    let n: u32 = c[4..].parse().map_err(|_| "bad volume")?;
                    format!("setPlayerCmd:vol:{}", n.min(100))
                }
                _ => return Err(format!("unknown remote command: {cmd}")),
            };
            client
                .get(format!("{base}/httpapi.asp?command={lp_cmd}"))
                .send()
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        Target::Upnp { ctrl_url, .. } => {
            let (action, args) = match cmd {
                "play" => ("Play", "<Speed>1</Speed>"),
                "pause" => ("Pause", ""),
                "skip" => ("Next", ""),
                "prev" => ("Previous", ""),
                _ => return Err(format!("unknown remote command: {cmd}")),
            };
            soap(client, &ctrl_url, action, args)
                .await
                .map(|_| ())
                .ok_or_else(|| format!("{action} failed"))
        }
    }
}

/// Background task: discover a renderer, poll it every second, emit
/// `remote://state` whenever anything changes.
pub fn start(app: tauri::AppHandle, ctl: RemoteCtl) {
    tauri::async_runtime::spawn(async move {
        let client = device_client();
        let mut last = RemoteState::default();
        let mut failures = 0u32;
        loop {
            let have_target = ctl.target.lock().await.is_some();
            if !have_target {
                let locations =
                    tauri::async_runtime::spawn_blocking(ssdp_search).await.unwrap_or_default();
                let mut best: Option<Target> = None;
                for loc in &locations {
                    if let Some(t) = probe_device(&client, loc).await {
                        let prefer = matches!(t, Target::LinkPlay { .. })
                            || t.name().to_lowercase().contains("wiim")
                            || t.name().to_lowercase().contains("speaker");
                        if best.is_none() || prefer {
                            best = Some(t);
                        }
                    }
                }
                if let Some(t) = best {
                    *ctl.target.lock().await = Some(t);
                } else {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    continue;
                }
            }

            let target = ctl.target.lock().await.clone();
            if let Some(t) = target {
                let polled = match &t {
                    Target::LinkPlay { base, name, pq_ctrl } => {
                        poll_linkplay(&client, base, name, pq_ctrl.is_some()).await
                    }
                    Target::Upnp { ctrl_url, name } => poll_upnp(&client, ctrl_url, name).await,
                };
                match polled {
                    Some(st) => {
                        failures = 0;
                        if st != last {
                            let _ = app.emit("remote://state", &st);
                            last = st;
                        }
                    }
                    None => {
                        failures += 1;
                        if failures >= 5 {
                            eprintln!("[remote] {} unreachable — rediscovering", t.name());
                            *ctl.target.lock().await = None;
                            failures = 0;
                            let _ = app.emit("remote://state", &RemoteState::default());
                            last = RemoteState::default();
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}
