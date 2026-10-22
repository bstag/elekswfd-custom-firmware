//! weather: WMO mapping and parsing of responses captured from the real APIs (tests/fixtures).

use serde_json::{Value, json};
use wfd_app::weather::*;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn wmo_matches_python() {
    for (c, want) in [
        (0, 1),
        (1, 1),
        (2, 2),
        (3, 2),
        (45, 2),
        (48, 2),
        (51, 3),
        (61, 3),
        (71, 3),
        (95, 3),
        (4, 3),
        (-1, 3),
    ] {
        assert_eq!(wmo(c), want, "code {c}");
    }
}

#[test]
fn condition_texts() {
    assert_eq!(condition(1), "Sunny");
    assert_eq!(condition(2), "Cloudy");
    assert_eq!(condition(3), "Rain");
    assert_eq!(condition(0), "");
}

#[test]
fn quote_like_urllib() {
    assert_eq!(quote("San Francisco"), "San%20Francisco");
    assert_eq!(quote("a/b_c.d-e~f"), "a/b_c.d-e~f");
    assert_eq!(quote("Zürich, CH&x=1"), "Z%C3%BCrich%2C%20CH%26x%3D1");
}

#[test]
fn parses_ip_api() {
    assert_eq!(
        parse_ip_api(&fixture("weather_ipapi.json")).unwrap(),
        (31.5504, -97.1609, "Waco".into())
    );
    let no_city = json!({"lat": 1.5, "lon": 2.0});
    assert_eq!(
        parse_ip_api(&no_city).unwrap(),
        (1.5, 2.0, "Auto (IP)".into())
    );
    assert!(parse_ip_api(&json!({})).is_err());
}

#[test]
fn parses_zippopotam() {
    let got = parse_zippopotam(&fixture("weather_zippopotam_76708.json")).unwrap();
    assert_eq!(got, (31.5765, -97.1786, "Waco TX".into()));
    assert!(parse_zippopotam(&json!({})).is_err());
}

#[test]
fn parses_geocoding() {
    let got = parse_geocoding(&fixture("weather_geocoding_sf.json")).unwrap();
    assert_eq!(got, (37.77493, -122.41942, "San Francisco".into()));
    assert!(parse_geocoding(&fixture("weather_geocoding_none.json")).is_err());
}

#[test]
fn parses_current() {
    assert_eq!(
        parse_current(&fixture("weather_forecast_f.json")).unwrap(),
        (1, 84)
    );
    assert_eq!(
        parse_current(&fixture("weather_forecast_c.json")).unwrap(),
        (1, 17)
    );
    // Python round(): ties to even; weather_code may come as a float
    let cur = |t: f64, c: Value| json!({"current": {"temperature_2m": t, "weather_code": c}});
    assert_eq!(parse_current(&cur(72.5, json!(61))).unwrap(), (3, 72));
    assert_eq!(parse_current(&cur(73.5, json!(3.0))).unwrap(), (2, 74));
    assert_eq!(parse_current(&cur(-0.5, json!(45))).unwrap(), (2, 0));
    assert_eq!(parse_current(&cur(-1.5, json!(0))).unwrap(), (1, -2));
    assert!(parse_current(&json!({"current": {"weather_code": 1}})).is_err());
}

#[test]
#[ignore = "live network: cargo test --test weather -- --ignored --nocapture"]
fn live_zip_76708() {
    let mut r = WeatherReader::new();
    let (geo, w) = r.run_once("76708", "fahrenheit").unwrap();
    println!("{geo:?}\n{w:?}");
    assert_eq!(w.label, "Waco TX");
    assert_eq!(w.unit, 0);
    assert!((1..=3).contains(&w.code));
    let (geo_c, w_c) = r.run_once(" 76708 ", "Celsius").unwrap();
    println!("{geo_c:?}\n{w_c:?}");
    assert_eq!((geo_c.lat, geo_c.lon, geo_c.unit), (geo.lat, geo.lon, 1));
}

fn day(day: &str, hi: i32, lo: i32) -> Day {
    Day { day: day.into(), hi, lo, scene: "clear".into() }
}

#[test]
fn parses_the_forecast() {
    let d = json!({"daily": {
        "time": ["2026-10-22", "2026-10-23", "2026-10-24", "2026-10-25"],
        "weather_code": [2, 61, 3, 0],
        "temperature_2m_max": [78.4, 81.5, 69.5, -0.5],
        "temperature_2m_min": [55.2, 60.9, 48.0, -2.5]}});
    let f = parse_forecast(&d);
    assert_eq!(f.len(), 4);
    assert_eq!((f[0].day.as_str(), f[0].hi, f[0].lo, f[0].scene.as_str()), ("Thu", 78, 55, "partly"));
    assert_eq!((f[1].day.as_str(), f[1].hi, f[1].scene.as_str()), ("Fri", 82, "rain"), "rounds half away from zero, like the clock");
    assert_eq!((f[3].hi, f[3].lo), (-1, -3));
    assert!(parse_forecast(&json!({"current": {}})).is_empty());
    let short = json!({"daily": {"time": ["2026-10-22"], "weather_code": [0], "temperature_2m_max": [70], "temperature_2m_min": [50]}});
    assert_eq!(parse_forecast(&short).len(), 1);
}

#[test]
fn forecast_text_matches_the_clock() {
    // the same cases as wfd-control/tests/test_extras.py (firmware extras::forecastText)
    let f = [day("Wed", 78, 55), day("Thu", 81, 60)];
    assert_eq!(forecast_text(0, &f, 0).as_deref(), Some("HI 78F"));
    assert_eq!(forecast_text(1, &f, 0).as_deref(), Some("LO 55F"));
    assert_eq!(forecast_text(2, &f, 0).as_deref(), Some("THU 81"));
    assert_eq!(forecast_text(5, &f, 0).as_deref(), Some("THU 81"), "cycles");
    let hot = [day("Sun", 104, 80), day("Mon", 99, 70)];
    assert_eq!(forecast_text(0, &hot, 0).as_deref(), Some("HI 104"), "no room for the unit");
    let cold = [day("Sun", -5, -12), day("Mon", 0, -3)];
    assert_eq!(forecast_text(1, &cold, 1).as_deref(), Some("LO -12"));
    let mild = [day("Sun", 3, -2), day("Mon", 4, 0)];
    assert_eq!(forecast_text(1, &mild, 1).as_deref(), Some("LO -2C"));
    assert_eq!(forecast_text(0, &f[..1], 0), None, "needs two days");
}
