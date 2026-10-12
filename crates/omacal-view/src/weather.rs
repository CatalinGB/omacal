//! The weather vocabulary the settings read depends on, kept Tauri-free.
//! Only the Omarchy widget's location file parser lives here; the fetch,
//! cache and geocoding stay on the desktop side.

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
