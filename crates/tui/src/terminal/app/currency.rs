//! Exchange rates for calc: cached rates at startup, then a background
//! refresh through the configured `[currency]` script, also available on demand.
use super::TerminalApp;
use app_core::currency;
use std::sync::mpsc;

#[cfg(test)]
pub(super) use note_session::rates::Fetched;
use note_session::rates::{RateJob, RateResult, RateService};
struct Refresh {
    rx: mpsc::Receiver<RateResult>,
    generation: u64,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Refresh {
    fn drop(&mut self) {
        // On editor exit, wait for process cleanup before the host exits.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[derive(Default)]
pub(super) struct CurrencyState {
    // Drops first: cancels service work before the host joins its worker.
    service: RateService,
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
        let mut state = Self::default();
        let startup = note_session::rates::RateStartupJob {
            config,
            cache_path: path,
            background_enabled: background_tasks_enabled,
        };
        let result = note_session::jobs::run_rate_startup(&startup);
        let (job, problem) = state.service.complete_startup(startup, result);
        if let Some(job) = job {
            state.launch(job);
        }
        (state, problem)
    }

    #[cfg(test)]
    fn fetch(&mut self, config: currency::CurrencyConfig, path: std::path::PathBuf) {
        if let Ok(job) = self.service.request_refresh(config, path) {
            self.launch(job);
        }
    }
    fn launch(&mut self, job: RateJob) {
        let generation = job.generation;
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = tx.send(note_session::jobs::run_rate_job(job));
        });
        self.refresh = Some(Refresh {
            generation,
            rx,
            worker: Some(worker),
        });
    }

    /// The finished refresh's result, if one is waiting to be applied.
    fn take_result(&mut self) -> Option<RateResult> {
        let result = match self.refresh.as_ref()?.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => RateResult {
                generation: self.refresh.as_ref()?.generation,
                result: Err("rates worker disconnected".into()),
            },
        };
        self.refresh = None;
        Some(result)
    }

    fn request_refresh(&mut self) -> Result<(), String> {
        self.service.ensure_idle()?;
        let config = app_core::config::load_currency_config()?
            .ok_or("currency: configure [currency] argv first")?;
        let job = self
            .service
            .request_refresh(config, currency::cache_path()?)?;
        self.launch(job);
        Ok(())
    }

    /// A refresh that never finishes.
    #[cfg(test)]
    pub(super) fn pending() -> Self {
        let (tx, rx) = mpsc::channel();
        std::mem::forget(tx);
        let mut service = RateService::default();
        let job = service
            .request_refresh(
                currency::CurrencyConfig {
                    argv: Vec::new(),
                    timeout_seconds: 1,
                },
                std::path::PathBuf::new(),
            )
            .unwrap();
        Self {
            service,
            refresh: Some(Refresh {
                generation: job.generation,
                rx,
                worker: None,
            }),
        }
    }

    #[cfg(test)]
    pub(super) fn with_result(result: Result<Fetched, String>) -> Self {
        let mut service = RateService::default();
        let job = service
            .request_refresh(
                currency::CurrencyConfig {
                    argv: Vec::new(),
                    timeout_seconds: 1,
                },
                std::path::PathBuf::new(),
            )
            .unwrap();
        let (tx, rx) = mpsc::channel();
        tx.send(RateResult {
            generation: job.generation,
            result,
        })
        .unwrap();
        Self {
            service,
            refresh: Some(Refresh {
                generation: job.generation,
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

    fn apply_currency_result(&mut self, result: RateResult) {
        self.render_state.dirty = true;
        let Some(result) = self.currency.service.complete(result) else {
            return;
        };
        let note_session::rates::RateCompletion {
            changed,
            as_of,
            save_error,
        } = match result {
            Ok(fetched) => fetched,
            Err(error) => {
                self.status = format!("exchange rates: {error}");
                return;
            }
        };
        if changed {
            self.session.invalidate_calc();
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
            .result
            .unwrap();
        assert_eq!(fetched.rates.rates["USD"], 1.5);
        assert!(fetched.save_error.is_none());
        assert_eq!(currency::load_cache(&path).unwrap(), Some(fetched.rates));
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
