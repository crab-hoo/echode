//! One record as the file's JSON line and as the text other sinks show.

use std::time::{SystemTime, UNIX_EPOCH};

use log::kv::{self, VisitSource, VisitValue};
use serde_json::{Map, Value};

/// A record owned past the `log` call, so the writer thread can take it.
#[derive(serde::Serialize)]
pub(crate) struct Line {
    pub(crate) time_ms: u64,
    pub(crate) pid: u32,
    pub(crate) level: &'static str,
    pub(crate) target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) thread: Option<String>,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub(crate) fields: Map<String, Value>,
}

impl Line {
    pub(crate) fn of(record: &log::Record<'_>) -> Self {
        let mut fields = Map::new();
        // A field that cannot be visited is left out; the message still logs.
        let _ = record.key_values().visit(&mut Fields(&mut fields));
        Self::new(
            level(record.level()),
            record.target().to_owned(),
            record.args().to_string(),
            fields,
        )
    }

    /// A line stamped now, on the calling thread.
    pub(crate) fn new(
        level: &'static str,
        target: String,
        message: String,
        fields: Map<String, Value>,
    ) -> Self {
        Self {
            time_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |time| {
                    u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
                }),
            pid: std::process::id(),
            level,
            target,
            thread: std::thread::current().name().map(str::to_owned),
            message,
            fields,
        }
    }

    /// The NDJSON line, with its newline.
    pub(crate) fn json(&self) -> Vec<u8> {
        let mut line = serde_json::to_vec(self).unwrap_or_default();
        line.push(b'\n');
        line
    }

    /// The JSON value a test capture keeps.
    pub(crate) fn value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// `message {fields}` for the unified log, which keeps its own time and level.
    pub(crate) fn message_and_fields(&self) -> String {
        if self.fields.is_empty() {
            return self.message.clone();
        }
        let fields = serde_json::to_string(&self.fields).unwrap_or_default();
        format!("{} {fields}", self.message)
    }

    /// `level target: message {fields}` for stderr.
    pub(crate) fn text(&self) -> String {
        format!(
            "{} {}: {}",
            self.level,
            self.target,
            self.message_and_fields()
        )
    }
}

fn level(level: log::Level) -> &'static str {
    match level {
        log::Level::Error => "error",
        log::Level::Warn => "warn",
        log::Level::Info => "info",
        log::Level::Debug => "debug",
        log::Level::Trace => "trace",
    }
}

struct Fields<'a>(&'a mut Map<String, Value>);

impl<'kvs> VisitSource<'kvs> for Fields<'_> {
    fn visit_pair(&mut self, key: kv::Key<'kvs>, value: kv::Value<'kvs>) -> Result<(), kv::Error> {
        let mut json = Json(Value::Null);
        value.visit(&mut json)?;
        self.0.insert(key.as_str().to_owned(), json.0);
        Ok(())
    }
}

/// A field's value in JSON. Integers wider than 64 bits and non-finite floats
/// become text, so a reader never meets a rounded or invalid number.
struct Json(Value);

impl<'v> VisitValue<'v> for Json {
    fn visit_any(&mut self, value: kv::Value<'_>) -> Result<(), kv::Error> {
        self.0 = Value::String(value.to_string());
        Ok(())
    }

    fn visit_null(&mut self) -> Result<(), kv::Error> {
        self.0 = Value::Null;
        Ok(())
    }

    fn visit_u64(&mut self, value: u64) -> Result<(), kv::Error> {
        self.0 = Value::from(value);
        Ok(())
    }

    fn visit_i64(&mut self, value: i64) -> Result<(), kv::Error> {
        self.0 = Value::from(value);
        Ok(())
    }

    fn visit_u128(&mut self, value: u128) -> Result<(), kv::Error> {
        self.0 =
            u64::try_from(value).map_or_else(|_| Value::String(value.to_string()), Value::from);
        Ok(())
    }

    fn visit_i128(&mut self, value: i128) -> Result<(), kv::Error> {
        self.0 =
            i64::try_from(value).map_or_else(|_| Value::String(value.to_string()), Value::from);
        Ok(())
    }

    fn visit_f64(&mut self, value: f64) -> Result<(), kv::Error> {
        self.0 = serde_json::Number::from_f64(value)
            .map_or_else(|| Value::String(value.to_string()), Value::Number);
        Ok(())
    }

    fn visit_bool(&mut self, value: bool) -> Result<(), kv::Error> {
        self.0 = Value::Bool(value);
        Ok(())
    }

    fn visit_str(&mut self, value: &str) -> Result<(), kv::Error> {
        self.0 = Value::String(value.to_owned());
        Ok(())
    }

    fn visit_char(&mut self, value: char) -> Result<(), kv::Error> {
        self.0 = Value::String(value.to_string());
        Ok(())
    }
}
