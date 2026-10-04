//! Structural validation of ```chart, ```plot and ```simulation specs, mirroring the
//! renderer's readers (`app/src/features/ask/visual/{chartSpec,plotSpec,simulationSpec}.ts`)
//! rule for rule: a spec accepted here is one the app draws, and one rejected here would
//! show "not drawn". Both sides run the same fixture cases (`fixtures/specs.json`).
//!
//! Lenient fields (labels, colours, step sizes, sample counts, durations) are clamped by
//! the renderer and never fail; only what makes a spec undrawable is an error.

use serde_json::{Map, Value};

use super::expr::{check_value, is_valid_name};
use super::VisualKind;

/// Largest plot or simulation spec drawn, in UTF-16 code units (JavaScript length).
pub const PLOT_MAX_CHARS: usize = 20_000;
pub const SIMULATION_MAX_CHARS: usize = 20_000;
pub const MAX_PARAMS: usize = 8;
pub const MAX_ITEMS: usize = 40;
pub const MAX_STATE: usize = 16;
pub const MAX_EVENTS: usize = 8;
pub const MAX_DRAW: usize = 40;
pub const MAX_READOUTS: usize = 8;
const CHART_KINDS: [&str; 5] = ["bar", "line", "area", "scatter", "pie"];
const MAX_CHART_POINTS: usize = 500;
const MAX_CHART_SERIES: usize = 12;
const MAX_BOUND: f64 = 1e9;
/// Variables of curves; parameters may never take these names.
const CURVE_VARIABLES: [&str; 2] = ["x", "t"];
const TIME: &str = "t";

/// Checks a visual's source as its renderer reads it. Kinds without a JSON spec pass.
pub fn validate_spec(kind: VisualKind, source: &str) -> Result<(), String> {
    match kind {
        VisualKind::Chart => validate_chart(source),
        VisualKind::Plot => validate_plot(source),
        VisualKind::Simulation => validate_simulation(source),
        VisualKind::Mermaid | VisualKind::Svg | VisualKind::Equation | VisualKind::Table => Ok(()),
    }
}

fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn finite(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn record(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

/// `value ?? fallback`: JavaScript's nullish default (absent or null takes the fallback).
fn or_default<'a>(value: Option<&'a Value>, fallback: &'a Value) -> &'a Value {
    match value {
        None | Some(Value::Null) => fallback,
        Some(v) => v,
    }
}

/// Collapsed, trimmed text of a string field ("" for anything else).
fn text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

fn type_of(item: &Map<String, Value>) -> String {
    item.get("type")
        .and_then(Value::as_str)
        .map(|t| t.trim().to_lowercase())
        .unwrap_or_default()
}

fn parse_object(source: &str, noun: &str) -> Result<Map<String, Value>, String> {
    let parsed: Value =
        serde_json::from_str(source).map_err(|_| format!("The {noun} is not valid JSON."))?;
    match parsed {
        Value::Object(map) => Ok(map),
        _ => Err(format!("The {noun} must be a JSON object.")),
    }
}

fn compile(value: Option<&Value>, names: &[&str], place: &str) -> Result<(), String> {
    check_value(value, names).map_err(|e| format!("{place}: {e}"))
}

fn pair(value: &Value, names: &[&str], place: &str) -> Result<(), String> {
    match value.as_array() {
        Some(items) if items.len() == 2 => {
            compile(items.first(), names, place)?;
            compile(items.get(1), names, place)
        }
        _ => Err(format!("{place} must be a pair [x, y].")),
    }
}

fn pair_at(value: Option<&Value>, names: &[&str], place: &str) -> Result<(), String> {
    pair(value.unwrap_or(&Value::Null), names, place)
}

// ---------------------------------------------------------------- chart

fn validate_chart(source: &str) -> Result<(), String> {
    let parsed: Value = serde_json::from_str(source)
        .map_err(|_| "The chart data is not valid JSON.".to_string())?;
    let chart = parsed
        .as_object()
        .ok_or_else(|| "The chart must be a JSON object.".to_string())?;
    let kind = chart
        .get("type")
        .and_then(Value::as_str)
        .map(|k| k.trim().to_lowercase())
        .unwrap_or_default();
    if !CHART_KINDS.contains(&kind.as_str()) {
        return Err(format!(
            "Unsupported chart type; use one of {}.",
            CHART_KINDS.join(", ")
        ));
    }
    let data = chart.get("data");
    if let (Some(Value::Array(rows)), Some(Value::String(_))) = (data, chart.get("xKey")) {
        let rows = rows
            .iter()
            .filter(|r| r.is_object())
            .take(MAX_CHART_POINTS)
            .count();
        let series = chart
            .get("series")
            .and_then(Value::as_array)
            .map(|s| {
                s.iter()
                    .filter_map(Value::as_object)
                    .filter(|s| s.get("key").is_some_and(Value::is_string))
                    .take(MAX_CHART_SERIES)
                    .count()
            })
            .unwrap_or(0);
        if rows == 0 {
            return Err("The chart has no data rows.".to_string());
        }
        if series == 0 {
            return Err("The chart names no series to plot.".to_string());
        }
        return Ok(());
    }
    if let Some(labels) = record(data)
        .and_then(|d| d.get("labels"))
        .and_then(Value::as_array)
    {
        let datasets = record(data)
            .and_then(|d| d.get("datasets"))
            .and_then(Value::as_array)
            .map(|d| d.iter().filter(|d| d.is_object()).count())
            .unwrap_or(0);
        if labels.is_empty() || datasets == 0 {
            return Err("The chart has no data.".to_string());
        }
        return Ok(());
    }
    Err("The chart needs \"xKey\", \"series\" and \"data\" rows.".to_string())
}

// ---------------------------------------------------------------- shared

fn axis(value: Option<&Value>, name: &str) -> Result<(), String> {
    let axis = record(value)
        .ok_or_else(|| format!("\"{name}\" must be an object with \"min\" and \"max\"."))?;
    let (Some(min), Some(max)) = (finite(axis.get("min")), finite(axis.get("max"))) else {
        return Err(format!(
            "\"{name}.min\" and \"{name}.max\" must be numbers."
        ));
    };
    if min >= max {
        return Err(format!("\"{name}.min\" must be less than \"{name}.max\"."));
    }
    if min.abs() > MAX_BOUND || max.abs() > MAX_BOUND {
        return Err(format!("\"{name}\" bounds are too large."));
    }
    Ok(())
}

/// The parameter names of a spec, after the renderer's checks. `reserved` are names that
/// variables of the spec already use.
fn params(value: Option<&Value>, reserved: &[&str]) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let list = value
        .as_array()
        .ok_or_else(|| "\"params\" must be a list.".to_string())?;
    if list.len() > MAX_PARAMS {
        return Err(format!("At most {MAX_PARAMS} parameters are supported."));
    }
    let mut names: Vec<String> = Vec::new();
    for (index, item) in list.iter().enumerate() {
        let param = item
            .as_object()
            .ok_or_else(|| format!("Parameter {} must be an object.", index + 1))?;
        let name = param
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if !is_valid_name(&name) || CURVE_VARIABLES.contains(&name.as_str()) {
            return Err(format!(
                "Parameter {} needs a \"name\" like \"v0\" (letters, digits, _; not x, t, pi, e or a function name).",
                index + 1
            ));
        }
        if names.contains(&name) {
            return Err(format!("Parameter \"{name}\" is defined twice."));
        }
        let (min, max) = (finite(param.get("min")), finite(param.get("max")));
        let (Some(min), Some(max)) = (min, max) else {
            return Err(format!(
                "Parameter \"{name}\" needs numeric \"min\" < \"max\"."
            ));
        };
        if min >= max {
            return Err(format!(
                "Parameter \"{name}\" needs numeric \"min\" < \"max\"."
            ));
        }
        if min.abs() > MAX_BOUND || max.abs() > MAX_BOUND {
            return Err(format!("Parameter \"{name}\" bounds are too large."));
        }
        if reserved.contains(&name.as_str()) {
            return Err(format!(
                "Parameter \"{name}\" clashes with a variable of the same name."
            ));
        }
        names.push(name);
    }
    Ok(names)
}

fn with<'a>(base: &'a [String], extra: &[&'a str]) -> Vec<&'a str> {
    base.iter()
        .map(String::as_str)
        .chain(extra.iter().copied())
        .collect()
}

// ---------------------------------------------------------------- plot

fn plot_item(value: &Value, index: usize, params: &[String]) -> Result<(), String> {
    let item = value
        .as_object()
        .ok_or_else(|| format!("Item {} must be an object.", index + 1))?;
    let kind = type_of(item);
    let place = format!(
        "Item {} ({})",
        index + 1,
        if kind.is_empty() { "no type" } else { &kind }
    );
    let names = with(params, &[]);
    match kind.as_str() {
        "function" => {
            let expr = match item.get("expr") {
                None | Some(Value::Null) => item.get("y"),
                some => some,
            };
            compile(expr, &with(params, &["x"]), &place)
        }
        "parametric" => {
            let curve = with(params, &["t"]);
            compile(item.get("x"), &curve, &format!("{place} x"))?;
            compile(item.get("y"), &curve, &format!("{place} y"))?;
            let default = serde_json::json!([0, 1]);
            pair(
                or_default(item.get("t"), &default),
                &names,
                &format!("{place} \"t\""),
            )
        }
        "point" => {
            let at = Value::Array(vec![
                item.get("x").cloned().unwrap_or(Value::Null),
                item.get("y").cloned().unwrap_or(Value::Null),
            ]);
            pair(&at, &names, &place)
        }
        "vector" | "segment" => {
            let origin = serde_json::json!([0, 0]);
            pair(
                or_default(item.get("from"), &origin),
                &names,
                &format!("{place} \"from\""),
            )?;
            pair_at(item.get("to"), &names, &format!("{place} \"to\""))
        }
        "label" => {
            pair_at(item.get("at"), &names, &format!("{place} \"at\""))?;
            if text(item.get("text")).is_empty() {
                return Err(format!("{place} needs \"text\"."));
            }
            Ok(())
        }
        _ => Err(format!(
            "{place}: unknown type; use function, parametric, point, vector, segment or label."
        )),
    }
}

fn validate_plot(source: &str) -> Result<(), String> {
    if js_len(source) > PLOT_MAX_CHARS {
        return Err(format!(
            "The plot spec is larger than {} KB.",
            PLOT_MAX_CHARS / 1000
        ));
    }
    let spec = parse_object(source, "plot spec")?;
    axis(spec.get("x"), "x")?;
    axis(spec.get("y"), "y")?;
    let params = params(spec.get("params"), &CURVE_VARIABLES)?;
    let items = match spec.get("items") {
        Some(Value::Array(items)) if !items.is_empty() => items,
        _ => return Err("The plot needs a non-empty \"items\" list.".to_string()),
    };
    if items.len() > MAX_ITEMS {
        return Err(format!("At most {MAX_ITEMS} items are supported."));
    }
    items
        .iter()
        .enumerate()
        .try_for_each(|(i, item)| plot_item(item, i, &params))
}

// ---------------------------------------------------------------- simulation

fn range(value: Option<&Value>, place: &str) -> Result<(), String> {
    let items = value
        .and_then(Value::as_array)
        .filter(|a| a.len() == 2)
        .ok_or_else(|| format!("{place} must be [min, max]."))?;
    let (Some(a), Some(b)) = (finite(items.first()), finite(items.get(1))) else {
        return Err(format!("{place} must be two numbers, min < max."));
    };
    if a >= b {
        return Err(format!("{place} must be two numbers, min < max."));
    }
    if a.abs() > MAX_BOUND || b.abs() > MAX_BOUND {
        return Err(format!("{place} is too large."));
    }
    Ok(())
}

fn draw_item(value: &Value, index: usize, names: &[&str]) -> Result<(), String> {
    let item = value
        .as_object()
        .ok_or_else(|| format!("Draw item {} must be an object.", index + 1))?;
    let kind = type_of(item);
    let place = format!(
        "Draw item {} ({})",
        index + 1,
        if kind.is_empty() { "no type" } else { &kind }
    );
    match kind.as_str() {
        "circle" => {
            pair_at(item.get("at"), names, &format!("{place} \"at\""))?;
            let radius = serde_json::json!(0.5);
            compile(
                Some(or_default(item.get("r"), &radius)),
                names,
                &format!("{place} \"r\""),
            )
        }
        "rect" => {
            pair_at(item.get("at"), names, &format!("{place} \"at\""))?;
            let unit = serde_json::json!([1, 1]);
            pair(
                or_default(item.get("size"), &unit),
                names,
                &format!("{place} \"size\""),
            )?;
            match item.get("angle") {
                None => Ok(()),
                some => compile(some, names, &format!("{place} \"angle\"")),
            }
        }
        "line" | "rod" | "spring" | "vector" => {
            pair_at(item.get("from"), names, &format!("{place} \"from\""))?;
            pair_at(item.get("to"), names, &format!("{place} \"to\""))
        }
        "trail" => pair_at(item.get("at"), names, &format!("{place} \"at\"")),
        "label" => {
            pair_at(item.get("at"), names, &format!("{place} \"at\""))?;
            if text(item.get("text")).is_empty() {
                return Err(format!("{place} needs \"text\"."));
            }
            Ok(())
        }
        _ => Err(format!(
            "{place}: unknown type; use circle, rect, line, rod, spring, vector, trail or label."
        )),
    }
}

fn validate_simulation(source: &str) -> Result<(), String> {
    if js_len(source) > SIMULATION_MAX_CHARS {
        return Err(format!(
            "The simulation spec is larger than {} KB.",
            SIMULATION_MAX_CHARS / 1000
        ));
    }
    let spec = parse_object(source, "simulation spec")?;
    let state = record(spec.get("state")).ok_or_else(|| {
        "\"state\" must be an object of initial values, e.g. {\"x\": \"0\"}.".to_string()
    })?;
    let state_names: Vec<String> = state.keys().cloned().collect();
    if state_names.is_empty() {
        return Err("\"state\" needs at least one variable.".to_string());
    }
    if state_names.len() > MAX_STATE {
        return Err(format!(
            "At most {MAX_STATE} state variables are supported."
        ));
    }
    if let Some(bad) = state_names
        .iter()
        .find(|n| !is_valid_name(n) || n.as_str() == TIME)
    {
        return Err(format!("\"{bad}\" cannot be a state variable name."));
    }
    let reserved = with(&state_names, &[TIME]);
    let params = params(spec.get("params"), &reserved)?;
    let param_names = with(&params, &[]);
    let mut all = param_names.clone();
    all.extend(state_names.iter().map(String::as_str));
    all.push(TIME);

    for name in &state_names {
        compile(
            state.get(name),
            &param_names,
            &format!("Initial value of \"{name}\""),
        )?;
    }

    let derivatives = record(spec.get("derivatives")).ok_or_else(|| {
        "\"derivatives\" must be an object, e.g. {\"x\": \"vx\", \"vx\": \"-k*x\"}.".to_string()
    })?;
    if let Some(key) = derivatives.keys().find(|k| !state_names.contains(k)) {
        return Err(format!(
            "\"derivatives\" names \"{key}\", which is not in \"state\"."
        ));
    }
    let zero = Value::String("0".to_string());
    for name in &state_names {
        compile(
            Some(derivatives.get(name).unwrap_or(&zero)),
            &all,
            &format!("Derivative of \"{name}\""),
        )?;
    }

    if let Some(events) = spec.get("events") {
        let events = events
            .as_array()
            .ok_or_else(|| "\"events\" must be a list.".to_string())?;
        if events.len() > MAX_EVENTS {
            return Err(format!("At most {MAX_EVENTS} events are supported."));
        }
        for (i, event) in events.iter().enumerate() {
            let place = format!("Event {}", i + 1);
            let (Some(event), Some(set)) = (
                event.as_object(),
                event.get("set").and_then(Value::as_object),
            ) else {
                return Err(format!("{place} needs \"when\" and \"set\"."));
            };
            compile(event.get("when"), &all, &format!("{place} \"when\""))?;
            for (key, value) in set {
                if !state_names.contains(key) {
                    return Err(format!(
                        "{place} sets \"{key}\", which is not in \"state\"."
                    ));
                }
                compile(Some(value), &all, &format!("{place} \"set.{key}\""))?;
            }
        }
    }

    if let Some(stop) = spec.get("stop") {
        compile(Some(stop), &all, "\"stop\"")?;
    }

    let view = record(spec.get("view")).ok_or_else(|| {
        "\"view\" must give the visible region, e.g. {\"x\": [0, 10], \"y\": [0, 5]}.".to_string()
    })?;
    range(view.get("x"), "\"view.x\"")?;
    range(view.get("y"), "\"view.y\"")?;

    let draw = match spec.get("draw") {
        Some(Value::Array(items)) if !items.is_empty() => items,
        _ => {
            return Err(
                "\"draw\" needs at least one item, e.g. a circle at [\"x\", \"y\"].".to_string(),
            )
        }
    };
    if draw.len() > MAX_DRAW {
        return Err(format!("At most {MAX_DRAW} draw items are supported."));
    }
    for (i, item) in draw.iter().enumerate() {
        draw_item(item, i, &all)?;
    }

    if let Some(readouts) = spec.get("readouts") {
        let readouts = readouts
            .as_array()
            .ok_or_else(|| "\"readouts\" must be a list.".to_string())?;
        for (i, readout) in readouts.iter().take(MAX_READOUTS).enumerate() {
            let readout = readout
                .as_object()
                .ok_or_else(|| format!("Readout {} must be an object.", i + 1))?;
            compile(readout.get("expr"), &all, &format!("Readout {}", i + 1))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One case of the fixture file shared with the frontend tests.
    #[derive(serde::Deserialize)]
    struct Case {
        name: String,
        kind: String,
        #[serde(default)]
        spec: Option<Value>,
        #[serde(default)]
        source: Option<String>,
        valid: bool,
    }

    fn kind_of(name: &str) -> VisualKind {
        match name {
            "chart" => VisualKind::Chart,
            "plot" => VisualKind::Plot,
            "simulation" => VisualKind::Simulation,
            other => panic!("unknown fixture kind {other}"),
        }
    }

    #[test]
    fn shared_fixture_cases_get_the_renderers_verdict() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("fixtures/specs.json")).expect("fixture file");
        assert!(cases.len() >= 40, "the fixture file lost cases");
        for case in cases {
            let source = match (&case.source, &case.spec) {
                (Some(s), _) => s.clone(),
                (None, Some(spec)) => serde_json::to_string(spec).expect("spec"),
                (None, None) => panic!("{}: no spec or source", case.name),
            };
            let verdict = validate_spec(kind_of(&case.kind), &source);
            assert_eq!(
                verdict.is_ok(),
                case.valid,
                "{} ({}): {:?}",
                case.name,
                case.kind,
                verdict
            );
        }
    }

    #[test]
    fn kinds_without_a_spec_pass() {
        assert!(validate_spec(VisualKind::Mermaid, "not json").is_ok());
        assert!(validate_spec(VisualKind::Equation, "x^2").is_ok());
    }

    #[test]
    fn oversized_specs_are_rejected_before_parsing() {
        let padding = " ".repeat(PLOT_MAX_CHARS);
        let source = format!(
            "{{\"x\":{{\"min\":0,\"max\":1}},\"y\":{{\"min\":0,\"max\":1}},\"items\":[{{\"type\":\"function\",\"expr\":\"x\"}}]}}{padding}"
        );
        let err = validate_spec(VisualKind::Plot, &source).unwrap_err();
        assert!(err.contains("larger than 20 KB"), "{err}");
    }
}
