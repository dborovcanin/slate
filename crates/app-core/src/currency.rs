//! Exchange rates for calc currency conversion. Rates come from a configured
//! script, are cached on disk, and are only read from memory during evaluation.
use crate::scripts::{run_script_raw, ScriptDefinition, ScriptInput, ScriptOutput, ScriptRequest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// `[currency]`: the script that fetches rates and how often to run it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrencyConfig {
    pub argv: Vec<String>,
    #[serde(default = "default_refresh_hours")]
    pub refresh_hours: u64,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}
fn default_refresh_hours() -> u64 {
    12
}
fn default_timeout() -> u64 {
    30
}

/// Parses the `[currency]` section; `None` when it is absent.
pub fn parse_currency_config(text: &str) -> Result<Option<CurrencyConfig>, String> {
    #[derive(Deserialize)]
    struct Root {
        currency: Option<CurrencyConfig>,
    }
    let root: Root = toml::from_str(text).map_err(|e| e.to_string())?;
    if let Some(config) = &root.currency {
        if config.argv.first().is_none_or(|s| s.trim().is_empty()) {
            return Err("currency: argv needs an executable".into());
        }
        if !(1..=3600).contains(&config.timeout_seconds) {
            return Err("currency: timeout_seconds must be 1..3600".into());
        }
        if !(1..=8760).contains(&config.refresh_hours) {
            return Err("currency: refresh_hours must be 1..8760".into());
        }
    }
    Ok(root.currency)
}

/// Units of each currency per one unit of `base`, as cached on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExchangeRates {
    pub base: String,
    pub rates: BTreeMap<String, f64>,
    /// Date the provider published the rates, if it said.
    #[serde(default)]
    pub as_of: Option<String>,
    /// Unix seconds when the rates were fetched.
    pub fetched_at: u64,
}

impl ExchangeRates {
    fn rate(&self, currency: &str) -> Option<f64> {
        if currency == self.base {
            return Some(1.0);
        }
        self.rates.get(currency).copied()
    }

    pub fn is_stale(&self, refresh_hours: u64, now: u64) -> bool {
        now.saturating_sub(self.fetched_at) >= refresh_hours.saturating_mul(3600)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RatesResponse {
    base: String,
    rates: BTreeMap<String, f64>,
    #[serde(default)]
    as_of: Option<String>,
}

fn is_currency_code(code: &str) -> bool {
    code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase())
}

fn parse_rates_response(stdout: &[u8], fetched_at: u64) -> Result<ExchangeRates, String> {
    let response: RatesResponse =
        serde_json::from_slice(stdout).map_err(|e| format!("invalid rates response: {e}"))?;
    let base = response.base.to_ascii_uppercase();
    if !is_currency_code(&base) {
        return Err(format!("invalid base currency: {}", response.base));
    }
    let mut rates = BTreeMap::new();
    for (code, rate) in response.rates {
        let code = code.to_ascii_uppercase();
        if !is_currency_code(&code) {
            return Err(format!("invalid currency code: {code}"));
        }
        if !rate.is_finite() || rate <= 0.0 {
            return Err(format!("invalid rate for {code}: {rate}"));
        }
        rates.insert(code, rate);
    }
    if rates.is_empty() {
        return Err("rates response has no rates".into());
    }
    Ok(ExchangeRates {
        base,
        rates,
        as_of: response.as_of,
        fetched_at,
    })
}

pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Runs the rates script. Blocking: call on a background thread.
pub fn fetch_rates(
    config: &CurrencyConfig,
    cancel: Arc<AtomicBool>,
) -> Result<ExchangeRates, String> {
    let script = ScriptDefinition {
        argv: config.argv.clone(),
        input: ScriptInput::None,
        output: ScriptOutput::Message,
        timeout_seconds: config.timeout_seconds,
    };
    let request = ScriptRequest {
        version: 1,
        args: Vec::new(),
        text: String::new(),
        note_id: None,
    };
    let stdout = run_script_raw(&script, &request, cancel)?;
    parse_rates_response(&stdout, now_unix_seconds())
}

pub fn cache_path() -> Result<PathBuf, String> {
    Ok(crate::data_dir()?.join("exchange_rates.json"))
}

/// Reads cached rates; `None` when there is no cache yet.
pub fn load_cache(path: &Path) -> Result<Option<ExchangeRates>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("invalid rates cache {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

pub fn save_cache(path: &Path, rates: &ExchangeRates) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(rates).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

static INSTALLED: RwLock<Option<Arc<ExchangeRates>>> = RwLock::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Makes `rates` the ones calc evaluation uses.
pub fn install(rates: ExchangeRates) {
    if let Ok(mut installed) = INSTALLED.write() {
        *installed = Some(Arc::new(rates));
        GENERATION.fetch_add(1, Ordering::AcqRel);
    }
}

pub fn installed() -> Option<Arc<ExchangeRates>> {
    INSTALLED.read().ok().and_then(|rates| rates.clone())
}

/// Changes whenever installed rates change, so cached calc results can tell.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// fend's exchange-rate hook. Reads installed rates only; never fetches.
pub(crate) struct InstalledRates;

impl fend_core::ExchangeRateFnV2 for InstalledRates {
    fn relative_to_base_currency(
        &self,
        currency: &str,
        _options: &fend_core::ExchangeRateFnV2Options,
    ) -> Result<f64, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let rates = INSTALLED.read().map_err(|_| "exchange rates unavailable")?;
        rates
            .as_ref()
            .and_then(|rates| rates.rate(currency))
            .ok_or_else(|| format!("no exchange rate for {currency}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_is_optional_and_validated() {
        assert!(parse_currency_config("[theme]\naccent='auto'")
            .unwrap()
            .is_none());
        let config = parse_currency_config("[currency]\nargv=['rates.py']")
            .unwrap()
            .unwrap();
        assert_eq!((config.refresh_hours, config.timeout_seconds), (12, 30));
        assert!(parse_currency_config("[currency]\nargv=[]").is_err());
        assert!(parse_currency_config("[currency]\nargv=['x']\nrefresh_hours=0").is_err());
        assert!(parse_currency_config("[currency]\nargv=['x']\ntypo=1").is_err());
    }

    #[test]
    fn response_is_normalized_and_validated() {
        let rates = parse_rates_response(
            br#"{"base":"eur","rates":{"usd":1.25},"as_of":"2026-10-09"}"#,
            7,
        )
        .unwrap();
        assert_eq!(rates.base, "EUR");
        assert_eq!(rates.rate("USD"), Some(1.25));
        assert_eq!(rates.rate("EUR"), Some(1.0));
        assert_eq!(rates.rate("GBP"), None);
        assert_eq!(rates.fetched_at, 7);
        for bad in [
            r#"{"base":"EUR","rates":{}}"#,
            r#"{"base":"EURO","rates":{"USD":1.1}}"#,
            r#"{"base":"EUR","rates":{"USD":-1}}"#,
            r#"{"base":"EUR","rates":{"US":1.1}}"#,
            r#"{"base":"EUR","rates":{"USD":1.1},"extra":1}"#,
        ] {
            assert!(parse_rates_response(bad.as_bytes(), 0).is_err(), "{bad}");
        }
    }

    #[test]
    fn staleness_uses_refresh_hours() {
        let rates = parse_rates_response(br#"{"base":"EUR","rates":{"USD":1.1}}"#, 1000).unwrap();
        assert!(!rates.is_stale(1, 1000 + 3599));
        assert!(rates.is_stale(1, 1000 + 3600));
    }

    #[test]
    fn cache_round_trips() {
        let dir = std::env::temp_dir().join(format!("slate-rates-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("exchange_rates.json");
        assert_eq!(load_cache(&path).unwrap(), None);
        let rates = parse_rates_response(br#"{"base":"EUR","rates":{"USD":1.1}}"#, 5).unwrap();
        save_cache(&path, &rates).unwrap();
        assert_eq!(load_cache(&path).unwrap(), Some(rates));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn fetch_runs_the_script() {
        let config = CurrencyConfig {
            argv: vec![
                "sh".into(),
                "-c".into(),
                r#"printf '{"base":"EUR","rates":{"USD":1.1}}'"#.into(),
            ],
            refresh_hours: 12,
            timeout_seconds: 5,
        };
        let rates = fetch_rates(&config, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(rates.rate("USD"), Some(1.1));
    }
}
