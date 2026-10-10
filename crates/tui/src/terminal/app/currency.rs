//! Exchange rates for calc: cached rates at startup, then a background
//! refresh through the configured `[currency]` script when they are stale.
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
    /// Installs cached rates, then starts a refresh when they are missing or
    /// older than `refresh_hours`. Returns a startup problem to report.
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
        // A broken cache is refetched, not fatal.
        let mut problem = None;
        match currency::load_cache(&path) {
            Ok(Some(rates)) => currency::install(rates),
            Ok(None) => {}
            Err(error) => problem = Some(format!("currency: {error}")),
        }
        let fresh = currency::installed().is_some_and(|rates| {
            !rates.is_stale(config.refresh_hours, currency::now_unix_seconds())
        });
        if fresh || !background_tasks_enabled {
            return (Self::default(), problem);
        }
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
        let refresh = Refresh {
            cancel,
            rx,
            worker: Some(worker),
        };
        (
            Self {
                refresh: Some(refresh),
            },
            problem,
        )
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
    pub(super) fn poll_currency_refresh(&mut self) {
        // Like script results, wait until no dialog or command bar is open.
        if super::scripts::mode_name(self.mode).is_none() {
            return;
        }
        let Some(refresh) = &self.currency.refresh else {
            return;
        };
        let result = match refresh.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("rates worker disconnected".into()),
        };
        self.currency.refresh = None;
        self.render_state.dirty = true;
        match result {
            Ok(Fetched { rates, save_error }) => {
                let as_of = rates.as_of.clone();
                currency::install(rates);
                self.recompute_calc_whole_note();
                self.status = match (save_error, as_of) {
                    (Some(error), _) => format!("exchange rates updated; not cached: {error}"),
                    (None, Some(as_of)) => format!("exchange rates updated ({as_of})"),
                    (None, None) => "exchange rates updated".into(),
                };
            }
            Err(error) => self.status = format!("exchange rates: {error}"),
        }
    }
}
