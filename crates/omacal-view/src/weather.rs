//! The weather vocabulary the settings read depends on, kept Tauri-free.
//!
//! The forecast fetch and cache stay on the desktop side; what lives here is
//! the Omarchy widget's location-file parser, the geocoder's pure parsers,
//! and the one write body both surfaces share — setting the forecast's place
//! from Settings (#117). The desktop wrapper keeps only its "refetch now"
//! side-effect.

use sqlx::SqlitePool;

/// The settings row holding the widget's location file.
pub const LOCATION_KEY: &str = "weather_location";

/// The Omarchy widget's location file, when this is an Omarchy machine and
/// the user has set one: `{name, latitude, longitude}` — coordinates
/// optional, a bare name geocodes below.
pub fn parse_omarchy_location(raw: &str) -> Option<(Option<(f64, f64)>, String)> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let name = v.get("name")?.as_str()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let coords = match (
        v.get("latitude").and_then(|x| x.as_f64()),
        v.get("longitude").and_then(|x| x.as_f64()),
    ) {
        (Some(lat), Some(lon)) => Some((lat, lon)),
        _ => None,
    };
    Some((coords, name))
}

/// Percent-encode a query value for Open-Meteo's geocoder, RFC 3986's
/// unreserved set left alone. Hand-rolled rather than pulling a dependency
/// in for one call, the way the desktop weather module always did it.
pub fn urlencoding_encode(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                vec![c.to_string()]
            } else {
                c.to_string().bytes().map(|b| format!("%{b:02X}")).collect()
            }
        })
        .collect()
}

/// First hit of Open-Meteo's geocoder: `(latitude, longitude, name)`.
pub fn parse_geocoded_place(raw: &str) -> Option<(f64, f64, String)> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let hit = v.get("results")?.get(0)?;
    let name = hit.get("name")?.as_str()?.trim().to_string();
    Some((hit.get("latitude")?.as_f64()?, hit.get("longitude")?.as_f64()?, name))
}

/// Sets the place the forecast is for, from OmaCal's Settings (#117), or
/// clears it with `None`.
///
/// A typed name is looked up once, here, with Open-Meteo's geocoder, and
/// stored as the name it resolved to with its coordinates — so a misspelled
/// city is refused at the field, where it can be fixed, rather than silently
/// leaving the forecast where it was. Answers with the settings now in force.
pub async fn set_weather_location(
    pool: &SqlitePool,
    name: Option<&str>,
) -> Result<crate::settings::AppSettings, String> {
    let wanted = name.map(str::trim).filter(|n| !n.is_empty());
    let stored = match wanted {
        None => String::new(),
        Some(n) => {
            let url = format!(
                "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=en&format=json",
                urlencoding_encode(n)
            );
            let raw = geocode_get(&url)
                .await
                .map_err(|_| "Could not look that place up — check the connection and try again.".to_string())?;
            let (lat, lon, resolved) = parse_geocoded_place(&raw)
                .ok_or_else(|| format!("No place called “{n}” was found. Try the city's name in English."))?;
            serde_json::json!({ "name": resolved, "latitude": lat, "longitude": lon }).to_string()
        }
    };
    crate::settings::write(pool, LOCATION_KEY, &stored)
        .await
        .map_err(|e| crate::errors::user_facing(&e))?;
    Ok(crate::settings::read_settings(pool).await)
}

/// One GET for the geocoder. Split out so the write body reads as the rule it
/// is, and so a test could stand a server in front of it.
async fn geocode_get(url: &str) -> anyhow::Result<String> {
    use anyhow::Context;
    let resp = reqwest::Client::builder()
        .user_agent(concat!("omacal/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()?
        .get(url)
        .send()
        .await
        .context("geocoder unreachable")?;
    if !resp.status().is_success() {
        anyhow::bail!("geocoder answered {}", resp.status());
    }
    Ok(resp.text().await?)
}
