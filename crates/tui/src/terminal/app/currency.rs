//! Exchange rates for calc: cached rates at startup, then a background
//! refresh through the configured `[currency]` script, also available on demand.
use super::TerminalApp;
use app_core::currency::{self, ExchangeRates};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};

pub(super) struct Fetched {
    pub(super) rates: ExchangeRates,
    /// The rates work, but could not be cached for the next start.
    pub(super) save_error: Option<String>,
}
struct Refresh {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<Fetched, String>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Refresh {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        // On editor exit, wait for process cleanup before the host exits.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[derive(Default)]
pub(super) struct CurrencyState {
    refresh: Option<Refresh>,
}
impl CurrencyState {
    /// Installs cached rates, then starts one refresh. Returns a startup problem.
    pub(super) fn start(background_tasks_enabled: bool) -> (Self, Option<String>) {
        let config = match app_core::config::load_currency_config() {
            Ok(Some(config)) => config,
            Ok(None) => return (Self::default(), None),
            Err(error) => return (Self::default(), Some(format!("currency: {error}"))),
        };
        let path = match currency::cache_path() {
            Ok(path) => path,
            Err(error) => return (Self::default(), Some(format!("currency: {error}"))),
        };
        Self::start_with_config(config, path, background_tasks_enabled)
    }

    pub(super) fn start_with_config(
        config: currency::CurrencyConfig,
        path: std::path::PathBuf,
        background_tasks_enabled: bool,
    ) -> (Self, Option<String>) {
        // A broken cache is refetched, not fatal.
        let mut problem = None;
        match currency::load_cache(&path) {
            Ok(Some(rates)) => {
                currency::install(rates);
            }
            Ok(None) => {}
            Err(error) => problem = Some(format!("currency: {error}")),
        }
        let mut state = Self::default();
        if background_tasks_enabled {
            state.fetch(config, path);
        }
        (state, problem)
    }

    fn fetch(&mut self, config: currency::CurrencyConfig, path: std::path::PathBuf) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            let result = currency::fetch_rates(&config, worker_cancel).map(|rates| Fetched {
                save_error: currency::save_cache(&path, &rates).err(),
                rates,
            });
            let _ = tx.send(result);
        });
        self.refresh = Some(Refresh {
            cancel,
            rx,
            worker: Some(worker),
        });
    }

    /// The finished refresh's result, if one is waiting to be applied.
    fn take_result(&mut self) -> Option<Result<Fetched, String>> {
        let result = match self.refresh.as_ref()?.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("rates worker disconnected".into()),
        };
        self.refresh = None;
        Some(result)
    }

    fn request_refresh(&mut self) -> Result<(), String> {
        if self.refresh.is_some() {
            return Err("exchange rates refresh already running".into());
        }
        let config = app_core::config::load_currency_config()?
            .ok_or("currency: configure [currency] argv first")?;
        self.fetch(config, currency::cache_path()?);
        Ok(())
    }

    /// A refresh that never finishes.
    #[cfg(test)]
    pub(super) fn pending() -> Self {
        let (tx, rx) = mpsc::channel();
        std::mem::forget(tx);
        Self {
            refresh: Some(Refresh {
                cancel: Arc::new(AtomicBool::new(false)),
                rx,
                worker: None,
            }),
        }
    }

    #[cfg(test)]
    pub(super) fn with_result(result: Result<Fetched, String>) -> Self {
        let (tx, rx) = mpsc::channel();
        tx.send(result).unwrap();
        Self {
            refresh: Some(Refresh {
                cancel: Arc::new(AtomicBool::new(false)),
                rx,
                worker: None,
            }),
        }
    }
}

impl TerminalApp {
    pub(super) fn refresh_currency(&mut self) {
        // A refresh that finished while the command bar was open is applied
        // now rather than reported as still running.
        if let Some(result) = self.currency.take_result() {
            self.apply_currency_result(result);
            return;
        }
        self.status = match self.currency.request_refresh() {
            Ok(()) => "refreshing exchange rates".into(),
            Err(error) => error,
        };
    }

    pub(super) fn poll_currency_refresh(&mut self) {
        // Like script results, wait until no dialog or command bar is open.
        if super::scripts::mode_name(self.mode).is_none() {
            return;
        }
        if let Some(result) = self.currency.take_result() {
            self.apply_currency_result(result);
        }
    }

    fn apply_currency_result(&mut self, result: Result<Fetched, String>) {
        self.render_state.dirty = true;
        let Fetched { rates, save_error } = match result {
            Ok(fetched) => fetched,
            Err(error) => {
                self.status = format!("exchange rates: {error}");
                return;
            }
        };
        let as_of = rates.as_of.clone();
        // Unchanged rates leave every calc result valid; skip re-evaluating.
        let changed = currency::install(rates);
        if changed {
            if let Ok(mut index) = self.cross_note_var_index.lock() {
                index.invalidate_calculations();
            }
            // Discard preparation made with the previous rates/exports.
            self.calc_workers.range_context_build = None;
            if !self.start_viewport_calc_preparation() {
                self.recompute_calc_whole_note();
            } else {
                self.clear_calc_cache();
            }
        }
        let outcome = if changed { "updated" } else { "unchanged" };
        self.status = match (save_error, as_of) {
            (Some(error), _) => format!("exchange rates {outcome}; not cached: {error}"),
            (None, Some(as_of)) => format!("exchange rates {outcome} ({as_of})"),
            (None, None) => format!("exchange rates {outcome}"),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn worker_fetches_once_and_rejects_overlapping_requests() {
        let dir = std::env::temp_dir().join(format!("slate-refresh-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("exchange_rates.json");
        let config = currency::CurrencyConfig {
            argv: vec![
                "sh".into(),
                "-c".into(),
                r#"printf '{"base":"EUR","rates":{"USD":1.5}}'"#.into(),
            ],
            timeout_seconds: 5,
        };
        let mut state = CurrencyState::default();
        state.fetch(config, path.clone());
        assert_eq!(
            state.request_refresh().unwrap_err(),
            "exchange rates refresh already running"
        );
        let fetched = state
            .refresh
            .as_ref()
            .unwrap()
            .rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(fetched.rates.rates["USD"], 1.5);
        assert!(fetched.save_error.is_none());
        assert_eq!(currency::load_cache(&path).unwrap(), Some(fetched.rates));
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
