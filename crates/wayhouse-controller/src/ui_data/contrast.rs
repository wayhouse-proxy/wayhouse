//! The ten theme checks of `docs/design-system/themes.md`, a port of
//! `crates/wayhouse-ui/web/src/lib/theme/validate.ts` and `color.ts`; the shared cases in that
//! directory's `fixtures.json` must give the same failed checks here (see the tests).
//!
//! Runs on the node that takes a theme write, never when a replica applies the entry: the
//! colour maths uses `powf`, `cbrt` and trigonometry, whose last digits may differ between CPU
//! architectures, and a mixed fleet must not disagree on what a log entry did.

use serde::Serialize;
use serde_json::{Map, Value};

type Rgb = [f64; 3];

/// Smallest CIEDE2000 difference between two colours that must stay apart (checks 8, 9, 10).
const MIN_DELTA_E: f64 = 10.0;
const TEXT_RATIO: f64 = 4.5;
const GRAPHIC_RATIO: f64 = 3.0;

const GROUNDS: [&str; 3] = ["bg", "surface", "surface-sunken"];
const STATES: [&str; 3] = ["good", "warn", "bad"];
const SERIES: [&str; 5] = ["series-1", "series-2", "series-3", "series-4", "series-5"];

#[derive(Clone, Copy, PartialEq)]
enum Cvd {
    Deuteranopia,
    Protanopia,
    Tritanopia,
}

impl Cvd {
    fn name(self) -> &'static str {
        match self {
            Cvd::Deuteranopia => "deuteranopia",
            Cvd::Protanopia => "protanopia",
            Cvd::Tritanopia => "tritanopia",
        }
    }
}

const ALL_VISIONS: [Option<Cvd>; 4] = [
    None,
    Some(Cvd::Deuteranopia),
    Some(Cvd::Protanopia),
    Some(Cvd::Tritanopia),
];

/// Values of the two built-in themes (`web/src/lib/theme/builtin.ts`; a test holds them to
/// `docs/design-system/tokens.css`).
const LIGHT: [(&str, &str); 23] = [
    ("bg", "#f3f1ee"),
    ("surface", "#fbfaf8"),
    ("surface-sunken", "#ebe8e3"),
    ("ink", "#1d1b18"),
    ("ink-muted", "#6a655d"),
    ("line", "#dcd8d1"),
    ("control-border", "#857f75"),
    ("rail", "#1d1f23"),
    ("rail-ink", "#e9e7e2"),
    ("rail-ink-muted", "#9b9a96"),
    ("rail-hover", "#2b2e34"),
    ("select", "#2b5cd6"),
    ("select-bg", "#e3eafb"),
    ("primary", "#1d1b18"),
    ("primary-ink", "#fbfaf8"),
    ("good", "#1b7654"),
    ("warn", "#b87400"),
    ("bad", "#c40e3b"),
    ("series-1", "#2b5ff3"),
    ("series-2", "#219a77"),
    ("series-3", "#984f85"),
    ("series-4", "#797e2a"),
    ("series-5", "#504d37"),
];
const DARK: [(&str, &str); 23] = [
    ("bg", "#16171a"),
    ("surface", "#1d1f23"),
    ("surface-sunken", "#202226"),
    ("ink", "#ecebe7"),
    ("ink-muted", "#9a968d"),
    ("line", "#2e3035"),
    ("control-border", "#7a7d85"),
    ("rail", "#0e0f11"),
    ("rail-ink", "#e9e7e2"),
    ("rail-ink-muted", "#96948f"),
    ("rail-hover", "#22252a"),
    ("select", "#8aa9ff"),
    ("select-bg", "#232b44"),
    ("primary", "#ecebe7"),
    ("primary-ink", "#16171a"),
    ("good", "#4bbfa6"),
    ("warn", "#fbc43b"),
    ("bad", "#e0645c"),
    ("series-1", "#6e8dff"),
    ("series-2", "#5ce1b4"),
    ("series-3", "#c1bedc"),
    ("series-4", "#d7c143"),
    ("series-5", "#98868e"),
];

/// One failed check, as `Failure` in `validate.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Failure {
    /// 1 to 10 as in themes.md; 0 for a malformed theme (unknown token, not `#rrggbb`).
    pub check: u8,
    pub pair: [String; 2],
    pub value: f64,
    pub needed: f64,
    pub message: String,
}

fn parse_hex(hex: &str) -> Option<Rgb> {
    let h = hex.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some([
        f64::from(n >> 16) / 255.0,
        f64::from((n >> 8) & 255) / 255.0,
        f64::from(n & 255) / 255.0,
    ])
}

fn to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(c: f64) -> f64 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn luminance(rgb: Rgb) -> f64 {
    let [r, g, b] = rgb.map(to_linear);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// WCAG contrast ratio, 1 to 21.
fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

// Machado, Oliveira and Fernandes 2009, severity 1.0, applied in linear RGB.
fn machado(kind: Cvd) -> [[f64; 3]; 3] {
    match kind {
        Cvd::Protanopia => [
            [0.152_286, 1.052_583, -0.204_868],
            [0.114_503, 0.786_281, 0.099_216],
            [-0.003_882, -0.048_116, 1.051_998],
        ],
        Cvd::Deuteranopia => [
            [0.367_322, 0.860_646, -0.227_968],
            [0.280_085, 0.672_501, 0.047_413],
            [-0.011_82, 0.042_94, 0.968_881],
        ],
        Cvd::Tritanopia => [
            [1.255_528, -0.076_749, -0.178_779],
            [-0.078_411, 0.930_809, 0.147_602],
            [0.004_733, 0.691_367, 0.303_9],
        ],
    }
}

fn simulate(rgb: Rgb, kind: Cvd) -> Rgb {
    let lin = rgb.map(to_linear);
    machado(kind).map(|row| {
        let v = row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2];
        to_srgb(v.clamp(0.0, 1.0))
    })
}

fn to_lab(rgb: Rgb) -> [f64; 3] {
    let [r, g, b] = rgb.map(to_linear);
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 colour difference (Sharma, Wu, Dalal 2005).
fn delta_e00(a: Rgb, b: Rgb) -> f64 {
    let [l1, a1, b1] = to_lab(a);
    let [l2, a2, b2] = to_lab(b);
    let rad = std::f64::consts::PI / 180.0;
    let (c1, c2) = (a1.hypot(b1), a2.hypot(b2));
    let cm7 = ((c1 + c2) / 2.0).powi(7);
    let g = 0.5 * (1.0 - (cm7 / (cm7 + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |y: f64, x: f64| {
        if x == 0.0 && y == 0.0 {
            return 0.0;
        }
        let h = y.atan2(x) / rad;
        if h < 0.0 {
            h + 360.0
        } else {
            h
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let mut dhp = 0.0;
    if c1p * c2p != 0.0 {
        dhp = h2p - h1p;
        if dhp > 180.0 {
            dhp -= 360.0;
        } else if dhp < -180.0 {
            dhp += 360.0;
        }
    }
    let d_hp = 2.0 * (c1p * c2p).sqrt() * (dhp * rad / 2.0).sin();
    let lbp = (l1 + l2) / 2.0;
    let cbp = (c1p + c2p) / 2.0;
    let mut hbp = h1p + h2p;
    if c1p * c2p != 0.0 {
        if (h1p - h2p).abs() > 180.0 {
            hbp += if h1p + h2p < 360.0 { 360.0 } else { -360.0 };
        }
        hbp /= 2.0;
    }
    let t = 1.0 - 0.17 * ((hbp - 30.0) * rad).cos()
        + 0.24 * (2.0 * hbp * rad).cos()
        + 0.32 * ((3.0 * hbp + 6.0) * rad).cos()
        - 0.2 * ((4.0 * hbp - 63.0) * rad).cos();
    let d_theta = 30.0 * (-(((hbp - 275.0) / 25.0).powi(2))).exp();
    let rc = 2.0 * (cbp.powi(7) / (cbp.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lbp - 50.0).powi(2) / (20.0 + (lbp - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cbp;
    let sh = 1.0 + 0.015 * cbp * t;
    let rt = -(2.0 * d_theta * rad).sin() * rc;
    ((dlp / sl).powi(2) + (dcp / sc).powi(2) + (d_hp / sh).powi(2) + rt * (dcp / sc) * (d_hp / sh))
        .sqrt()
}

fn builtin(base: &str) -> Option<&'static [(&'static str, &'static str); 23]> {
    match base {
        "light" => Some(&LIGHT),
        "dark" => Some(&DARK),
        _ => None,
    }
}

/// Runs the ten checks on a theme (`base` plus token overrides). `Err` for a `base` that is not
/// `light` or `dark`; the shape rules in [`super::validate`] refuse that earlier.
pub fn validate_theme(base: &str, overrides: &Map<String, Value>) -> Result<Vec<Failure>, String> {
    let defaults = builtin(base).ok_or("base must be \"light\" or \"dark\"")?;
    let mut out = Vec::new();
    for (k, v) in overrides {
        if !defaults.iter().any(|(name, _)| name == k) {
            out.push(malformed(k, format!("Unknown token \"{k}\".")));
        } else if !v.as_str().is_some_and(|s| parse_hex(s).is_some()) {
            let shown = v.as_str().map_or_else(|| v.to_string(), str::to_string);
            out.push(malformed(
                k,
                format!("{k} must be a #rrggbb colour, got \"{shown}\"."),
            ));
        }
    }
    if !out.is_empty() {
        return Ok(out);
    }

    let colour = |name: &str| -> Rgb {
        let hex = overrides
            .get(name)
            .and_then(Value::as_str)
            .or_else(|| defaults.iter().find(|(n, _)| *n == name).map(|(_, h)| *h))
            .expect("every checked token is a themable one");
        parse_hex(hex).expect("built-in and checked colours are #rrggbb")
    };
    for g in GROUNDS {
        ratio(&mut out, &colour, 1, "ink", g, TEXT_RATIO);
    }
    for g in GROUNDS {
        ratio(&mut out, &colour, 2, "ink-muted", g, TEXT_RATIO);
    }
    for f in ["rail-ink", "rail-ink-muted"] {
        for g in ["rail", "rail-hover"] {
            ratio(&mut out, &colour, 3, f, g, TEXT_RATIO);
        }
    }
    ratio(&mut out, &colour, 4, "select", "surface", TEXT_RATIO);
    ratio(&mut out, &colour, 4, "ink", "select-bg", TEXT_RATIO);
    ratio(&mut out, &colour, 5, "primary-ink", "primary", TEXT_RATIO);
    for s in STATES {
        for g in GROUNDS {
            // warn is only drawn as a mark or border, so it needs the graphic ratio (see validate.ts).
            let needed = if s == "warn" {
                GRAPHIC_RATIO
            } else {
                TEXT_RATIO
            };
            ratio(&mut out, &colour, 6, s, g, needed);
        }
    }
    for g in ["surface", "bg"] {
        ratio(&mut out, &colour, 7, "control-border", g, GRAPHIC_RATIO);
    }
    for (a, b) in pairs(&STATES) {
        apart(&mut out, &colour, 8, a, b, &ALL_VISIONS);
    }
    for s in SERIES {
        ratio(&mut out, &colour, 9, s, "surface", GRAPHIC_RATIO);
    }
    for (a, b) in pairs(&SERIES) {
        apart(&mut out, &colour, 9, a, b, &ALL_VISIONS);
    }
    for s in STATES {
        apart(&mut out, &colour, 10, "select", s, &[None]);
    }
    Ok(out)
}

fn ratio(
    out: &mut Vec<Failure>,
    colour: &dyn Fn(&str) -> Rgb,
    check: u8,
    fg: &str,
    bg: &str,
    needed: f64,
) {
    let value = contrast(colour(fg), colour(bg));
    if value < needed {
        out.push(Failure {
            check,
            pair: [fg.into(), bg.into()],
            value,
            needed,
            message: format!("{fg} on {bg} is {value:.2}:1, needs {needed}:1."),
        });
    }
}

fn malformed(token: &str, message: String) -> Failure {
    Failure {
        check: 0,
        pair: [token.into(), token.into()],
        value: 0.0,
        needed: 0.0,
        message,
    }
}

fn pairs<'a>(names: &[&'a str]) -> Vec<(&'a str, &'a str)> {
    names
        .iter()
        .enumerate()
        .flat_map(|(i, a)| names[i + 1..].iter().map(move |b| (*a, *b)))
        .collect()
}

/// Pushes one failure for the pair at the first vision where it is less than [`MIN_DELTA_E`] apart.
fn apart(
    out: &mut Vec<Failure>,
    colour: &dyn Fn(&str) -> Rgb,
    check: u8,
    a: &str,
    b: &str,
    visions: &[Option<Cvd>],
) {
    for v in visions {
        let seen = |name: &str| v.map_or_else(|| colour(name), |k| simulate(colour(name), k));
        let value = delta_e00(seen(a), seen(b));
        if value < MIN_DELTA_E {
            let vision = v.map_or("normal vision", Cvd::name);
            out.push(Failure {
                check,
                pair: [a.into(), b.into()],
                value,
                needed: MIN_DELTA_E,
                message: format!(
                    "{a} and {b} differ by {value:.1} ({vision}), need {MIN_DELTA_E}."
                ),
            });
            return;
        }
    }
}

/// The distinct failed check numbers, ascending (`failedChecks` in `validate.ts`).
pub fn failed_checks(failures: &[Failure]) -> Vec<u8> {
    let mut checks: Vec<u8> = failures.iter().map(|f| f.check).collect();
    checks.sort_unstable();
    checks.dedup();
    checks
}

/// The contrast verdict for a theme record that already passed the shape rules: `Err` carries
/// every failure.
pub fn check_record(value: &Value) -> Result<(), Vec<Failure>> {
    let base = value.get("base").and_then(Value::as_str).unwrap_or("");
    let empty = Map::new();
    let tokens = value
        .get("tokens")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    match validate_theme(base, tokens) {
        Ok(f) if f.is_empty() => Ok(()),
        Ok(f) => Err(f),
        Err(why) => Err(vec![malformed("base", why)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FIXTURES: &str = include_str!("../../../wayhouse-ui/web/src/lib/theme/fixtures.json");
    const TOKENS_CSS: &str = include_str!("../../../../docs/design-system/tokens.css");

    fn run(theme: &Value) -> Vec<Failure> {
        let tokens = theme["tokens"].as_object().unwrap();
        validate_theme(theme["base"].as_str().unwrap(), tokens).unwrap()
    }

    #[test]
    fn the_shared_fixtures_fail_the_same_checks_as_the_web_validator() {
        let cases: Vec<Value> = serde_json::from_str(FIXTURES).unwrap();
        assert!(cases.len() >= 12);
        for case in cases {
            let want: Vec<u8> = serde_json::from_value(case["failedChecks"].clone()).unwrap();
            let got = failed_checks(&run(&case["theme"]));
            assert_eq!(got, want, "{}", case["name"]);
        }
    }

    #[test]
    fn a_failure_names_the_pair_and_the_ratio_like_the_web_validator() {
        let f = run(&json!({"base": "light", "tokens": {"ink": "#bbbbbb"}}));
        assert_eq!(f[0].pair, ["ink".to_string(), "bg".to_string()]);
        let msg = &f[0].message;
        assert!(
            msg.starts_with("ink on bg is 1.") && msg.ends_with(":1, needs 4.5:1."),
            "{msg}"
        );
    }

    #[test]
    fn a_colour_pair_failure_names_the_vision_that_failed() {
        let f = run(&json!({"base": "light", "tokens": {"warn": "#c40e3b"}}));
        let f: Vec<_> = f.iter().filter(|f| f.check == 8).collect();
        assert_eq!(f.len(), 1);
        assert_eq!(
            f[0].message,
            "warn and bad differ by 0.0 (normal vision), need 10."
        );
    }

    #[test]
    fn a_base_other_than_light_or_dark_is_refused() {
        assert!(validate_theme("blue", &Map::new()).is_err());
    }

    /// `(name, #hex)` pairs of the `--name: #rrggbb` lines in the css block opened by `start`.
    fn css_block(start: &str) -> Vec<(String, String)> {
        let body = TOKENS_CSS.split(start).nth(1).expect("block");
        let body = &body[..body.find("\n}").expect("block end")];
        body.lines()
            .filter_map(|l| {
                let (name, rest) = l.trim().strip_prefix("--")?.split_once(':')?;
                let hex = rest.split(';').next()?.trim();
                parse_hex(hex).map(|_| (name.to_string(), hex.to_string()))
            })
            .collect()
    }

    #[test]
    fn the_built_in_values_are_the_ones_tokens_css_ships() {
        for (base, start, table) in [
            ("light", "\n:root {", &LIGHT),
            ("dark", ":root[data-theme=\"dark\"] {", &DARK),
        ] {
            let css = css_block(start);
            for (name, hex) in table {
                let shipped = css.iter().find(|(n, _)| n == name).map(|(_, h)| h.as_str());
                assert_eq!(shipped, Some(*hex), "{base} {name}");
            }
        }
    }
}
