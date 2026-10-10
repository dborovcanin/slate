//! Exchange rates for calc: cached rates at startup, then a background
//! refresh, as in the terminal app. The policy is `note_session::rates`.
use app_core::currency;
use note_session::rates::{RateJob, RateResult, RateService};
use std::sync::mpsc;

struct Refresh {
    generation: u64,
    rx: mpsc::Receiver<RateResult>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Refresh {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Default)]
pub struct Currency {
    // Drops first: cancels service work before the worker is joined.
    service: RateService,
    refresh: Option<Refresh>,
}

impl Currency {
    /// Install cached rates, then start one refresh. Call before the first
    /// calc so conversions work on open. Returns a startup problem.
    pub fn start(background_tasks: bool) -> (Self, Option<String>) {
        let config = match app_core::config::load_currency_config() {
            Ok(Some(config)) => config,
            Ok(None) => return (Self::default(), None),
            Err(error) => return (Self::default(), Some(format!("currency: {error}"))),
        };
        let path = match currency::cache_path() {
            Ok(path) => path,
            Err(error) => return (Self::default(), Some(format!("currency: {error}"))),
        };
        let mut state = Self::default();
        let startup = note_session::rates::RateStartupJob {
            config,
            cache_path: path,
            background_enabled: background_tasks,
        };
        let result = note_session::jobs::run_rate_startup(&startup);
        let (job, problem) = state.service.complete_startup(startup, result);
        if let Some(job) = job {
            state.launch(job);
        }
        (state, problem)
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

    /// Start a refresh now (`:currency refresh`).
    pub fn request_refresh(&mut self) -> Result<(), String> {
        self.service.ensure_idle()?;
        let config = app_core::config::load_currency_config()?
            .ok_or("currency: configure [currency] argv first")?;
        let job = self
            .service
            .request_refresh(config, currency::cache_path()?)?;
        self.launch(job);
        Ok(())
    }

    /// A finished refresh applied: `Some(status, changed)`.
    pub fn poll(&mut self) -> Option<(String, bool)> {
        let result = match self.refresh.as_ref()?.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => RateResult {
                generation: self.refresh.as_ref()?.generation,
                result: Err("rates worker disconnected".into()),
            },
        };
        self.refresh = None;
        let done = match self.service.complete(result)? {
            Ok(done) => done,
            Err(error) => return Some((format!("exchange rates: {error}"), false)),
        };
        let outcome = if done.changed { "updated" } else { "unchanged" };
        let status = match (done.save_error, done.as_of) {
            (Some(error), _) => format!("exchange rates {outcome}; not cached: {error}"),
            (None, Some(as_of)) => format!("exchange rates {outcome} ({as_of})"),
            (None, None) => format!("exchange rates {outcome}"),
        };
        Some((status, done.changed))
    }
}
