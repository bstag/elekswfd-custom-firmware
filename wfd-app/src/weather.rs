//! weather: location lookup + current conditions, ported from app.py (`resolve_location`,
//! `weather_reader`, `_wmo`, `_COND`). Network calls are thin wrappers around pure `parse_*`
//! functions so the parsing is tested against captured API responses (tests/weather.rs).

use std::time::Duration;

use serde_json::Value;
use ureq::Agent;

/// Current weather as the clock shows it. `code`: 1 sunny, 2 cloudy, 3 rain; `unit`: 1 = Celsius.
/// `scene`: the finer picture for the "weather" grid animation (`grid_anim::SCENES`), "" = unknown.
/// `forecast`: today and the next days (up to 4).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Weather {
    pub code: u8,
    pub temp: i32,
    pub unit: u8,
    pub label: String,
    pub cond: String,
    pub scene: String,
    pub forecast: Vec<Day>,
}

/// One forecast day: "Thu", high and low (rounded like the clock: half away from zero), scene.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Day {
    pub day: String,
    pub hi: i32,
    pub lo: i32,
    pub scene: String,
}

/// forecast `daily` -> up to 4 days (the clock reads the same arrays).
pub fn parse_forecast(d: &Value) -> Vec<Day> {
    let daily = &d["daily"];
    let col = |k: &str| daily[k].as_array().cloned().unwrap_or_default();
    let (days, hi, lo, codes) = (
        col("time"),
        col("temperature_2m_max"),
        col("temperature_2m_min"),
        col("weather_code"),
    );
    let n = days.len().min(hi.len()).min(lo.len()).min(codes.len()).min(4);
    (0..n)
        .map_while(|i| {
            let date = chrono::NaiveDate::parse_from_str(days[i].as_str()?, "%Y-%m-%d").ok()?;
            let code = codes[i].as_f64()? as i64;
            Some(Day {
                day: date.format("%a").to_string(),
                hi: hi[i].as_f64()?.round() as i32,
                lo: lo[i].as_f64()?.round() as i32,
                scene: crate::grid_anim::SCENES[crate::grid_anim::scene_from_wmo(code, true)].into(),
            })
        })
        .collect()
}

/// The forecast on the message line (firmware `extras::forecastText`), item 0..2 of the cycle:
/// "HI 78F", "LO 55F", "THU 81". None until two days are known.
pub fn forecast_text(item: u64, days: &[Day], unit: u8) -> Option<String> {
    if days.len() < 2 {
        return None;
    }
    let u = if unit == 1 { 'C' } else { 'F' };
    Some(match item % 3 {
        2 => format!("{} {}", days[1].day.to_uppercase(), days[1].hi),
        k => {
            let (lbl, t) = if k == 0 { ("HI", days[0].hi) } else { ("LO", days[0].lo) };
            let s = format!("{lbl} {t}{u}");
            if s.len() > 6 { format!("{lbl} {t}") } else { s }
        }
    })
}

/// Resolved location (rounded to 4 decimals like app.py's `geo` state) and unit (1 = Celsius).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Geo {
    pub lat: f64,
    pub lon: f64,
    pub unit: u8,
}

/// WMO weather code -> clock code (`_wmo`).
pub fn wmo(code: i64) -> u8 {
    match code {
        0 | 1 => 1,
        2 | 3 | 45 | 48 => 2,
        _ => 3,
    }
}

/// Clock code -> condition text (`_COND`); "" for codes `wmo` never produces.
pub fn condition(code: u8) -> &'static str {
    match code {
        1 => "Sunny",
        2 => "Cloudy",
        3 => "Rain",
        _ => "",
    }
}

fn agent() -> Agent {
    Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .into()
}

/// GET a URL as JSON with app.py's User-Agent (`_get`).
fn get_json(agent: &Agent, url: &str) -> Result<Value, String> {
    agent
        .get(url)
        .header("User-Agent", "wfd-clock")
        .call()
        .map_err(|e| format!("{url}: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| format!("{url}: {e}"))
}

/// `urllib.parse.quote(s)`: percent-encode UTF-8 bytes except letters, digits, `_.-~` and `/`.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A float as Python's `str()` writes it in an f-string (integral values keep ".0").
fn py_float(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e16 {
        format!("{v:.1}")
    } else {
        v.to_string()
    }
}

/// Python's `round(x, 4)` (exact decimal rounding of the binary value, ties to even).
fn round4(v: f64) -> f64 {
    format!("{v:.4}").parse().unwrap_or(v)
}

fn num(v: &Value, what: &str) -> Result<f64, String> {
    v.as_f64()
        .ok_or_else(|| format!("missing or non-numeric {what}"))
}

/// Number given as a JSON string (`float(p["latitude"])`).
fn str_num(v: &Value, what: &str) -> Result<f64, String> {
    v.as_str()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| format!("bad {what}"))
}

/// ip-api.com `?fields=lat,lon,city` -> (lat, lon, city or "Auto (IP)").
pub fn parse_ip_api(d: &Value) -> Result<(f64, f64, String), String> {
    let city = d.get("city").and_then(Value::as_str).unwrap_or("Auto (IP)");
    Ok((
        num(&d["lat"], "lat")?,
        num(&d["lon"], "lon")?,
        city.to_string(),
    ))
}

/// api.zippopotam.us/us/<zip> -> (lat, lon, "<place> <ST>").
pub fn parse_zippopotam(d: &Value) -> Result<(f64, f64, String), String> {
    let p = d["places"].get(0).ok_or("no places for that ZIP")?;
    let name = p["place name"].as_str().ok_or("no place name")?;
    let st = p["state abbreviation"].as_str().ok_or("no state")?;
    Ok((
        str_num(&p["latitude"], "latitude")?,
        str_num(&p["longitude"], "longitude")?,
        format!("{name} {st}"),
    ))
}

/// geocoding-api.open-meteo.com search -> (lat, lon, name) of the first result.
pub fn parse_geocoding(d: &Value) -> Result<(f64, f64, String), String> {
    let r = d["results"].get(0).ok_or("location not found")?;
    let name = r["name"].as_str().ok_or("no name")?;
    Ok((
        num(&r["latitude"], "latitude")?,
        num(&r["longitude"], "longitude")?,
        name.to_string(),
    ))
}

/// api.open-meteo.com forecast `current` -> (clock code, temperature rounded half-to-even).
pub fn parse_current(d: &Value) -> Result<(u8, i32), String> {
    let c = &d["current"];
    let wc = &c["weather_code"];
    // int(c["weather_code"]): accepts 3 or 3.0
    let code = wc
        .as_i64()
        .or_else(|| wc.as_f64().map(|f| f.trunc() as i64))
        .ok_or("no weather_code")?;
    let temp = num(&c["temperature_2m"], "temperature_2m")?;
    Ok((wmo(code), temp.round_ties_even() as i32))
}

/// forecast `current` -> the weather animation's scene (WMO code + `is_day`, day if missing).
pub fn parse_scene(d: &Value) -> Result<&'static str, String> {
    let c = &d["current"];
    let wc = &c["weather_code"];
    let code = wc
        .as_i64()
        .or_else(|| wc.as_f64().map(|f| f.trunc() as i64))
        .ok_or("no weather_code")?;
    let day = c["is_day"].as_i64().or_else(|| c["is_day"].as_f64().map(|f| f as i64)) != Some(0);
    Ok(crate::grid_anim::SCENES[crate::grid_anim::scene_from_wmo(code, day)])
}

fn resolve_with(agent: &Agent, loc: &str) -> Result<(f64, f64, String), String> {
    if loc.is_empty() {
        return parse_ip_api(&get_json(
            agent,
            "http://ip-api.com/json/?fields=lat,lon,city",
        )?);
    }
    if loc.len() == 5 && loc.bytes().all(|b| b.is_ascii_digit()) {
        return parse_zippopotam(&get_json(
            agent,
            &format!("https://api.zippopotam.us/us/{loc}"),
        )?);
    }
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1",
        quote(loc)
    );
    parse_geocoding(&get_json(agent, &url)?)
}

/// Blank -> by IP (ip-api), 5-digit US ZIP -> zippopotam, anything else -> open-meteo geocoding.
pub fn resolve_location(loc: &str) -> Result<(f64, f64, String), String> {
    resolve_with(&agent(), loc)
}

/// (clock code, rounded temperature, scene, forecast) at a location.
fn fetch_with(agent: &Agent, lat: f64, lon: f64, celsius: bool) -> Result<(u8, i32, &'static str, Vec<Day>), String> {
    let uname = if celsius { "celsius" } else { "fahrenheit" };
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}\
         &current=temperature_2m,weather_code,is_day&temperature_unit={uname}\
         &daily=weather_code,temperature_2m_max,temperature_2m_min&forecast_days=4&timezone=auto",
        py_float(lat),
        py_float(lon)
    );
    let d = get_json(agent, &url)?;
    let (code, temp) = parse_current(&d)?;
    Ok((code, temp, parse_scene(&d)?, parse_forecast(&d)))
}

/// Current (clock code, rounded temperature) at a location.
pub fn fetch_current(lat: f64, lon: f64, celsius: bool) -> Result<(u8, i32), String> {
    fetch_with(&agent(), lat, lon, celsius).map(|(code, temp, _, _)| (code, temp))
}

/// One `weather_reader` iteration at a time; re-resolves the location only when it changes.
pub struct WeatherReader {
    agent: Agent,
    cur_loc: Option<String>,
    place: Option<(f64, f64, String)>,
}

impl Default for WeatherReader {
    fn default() -> Self {
        Self::new()
    }
}

impl WeatherReader {
    pub fn new() -> Self {
        Self {
            agent: agent(),
            cur_loc: None,
            place: None,
        }
    }

    /// `location`: the clock's ZIP / place (blank = by IP); `units`: "celsius"/"fahrenheit"
    /// (anything starting with "c" is Celsius). The caller polls this every 900 s.
    pub fn run_once(&mut self, location: &str, units: &str) -> Result<(Geo, Weather), String> {
        let loc = location.trim();
        let celsius = units.to_lowercase().starts_with('c');
        if self.cur_loc.as_deref() != Some(loc) || self.place.is_none() {
            self.place = Some(resolve_with(&self.agent, loc)?);
            self.cur_loc = Some(loc.to_string());
        }
        let (lat, lon, label) = self.place.clone().expect("resolved above");
        let (code, temp, scene, forecast) = fetch_with(&self.agent, lat, lon, celsius)?;
        let unit = u8::from(celsius);
        let geo = Geo {
            lat: round4(lat),
            lon: round4(lon),
            unit,
        };
        let weather = Weather {
            code,
            temp,
            unit,
            label,
            cond: condition(code).to_string(),
            scene: scene.to_string(),
            forecast,
        };
        Ok((geo, weather))
    }
}
