//! Application-wide refresh policy, independent of note lifetimes and executors.
use app_core::currency::{self, CurrencyConfig, ExchangeRates};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
#[derive(Default)]
pub struct RateService {
    generation: u64,
    active: Option<(u64, Arc<AtomicBool>)>,
    publication: Arc<Mutex<u64>>,
}
pub struct RateJob {
    pub generation: u64,
    pub config: CurrencyConfig,
    pub cache_path: PathBuf,
    pub cancel: Arc<AtomicBool>,
    publication: Arc<Mutex<u64>>,
}
pub struct Fetched {
    pub rates: ExchangeRates,
    pub save_error: Option<String>,
}
pub struct RateResult {
    pub generation: u64,
    pub result: Result<Fetched, String>,
}
pub struct RateCompletion {
    pub changed: bool,
    pub as_of: Option<String>,
    pub save_error: Option<String>,
}
pub struct RateStartupJob {
    pub config: CurrencyConfig,
    pub cache_path: PathBuf,
    pub background_enabled: bool,
}
pub struct RateStartupResult {
    pub cached: Result<Option<ExchangeRates>, String>,
}
/// Cached startup reads are synchronous when the host chooses, before its initial calc.
pub fn run_rate_startup(job: &RateStartupJob) -> RateStartupResult {
    RateStartupResult {
        cached: currency::load_cache(&job.cache_path),
    }
}
/// Shared runner; only the host chooses a thread and waits for process cleanup.
pub fn run_rate_job(job: RateJob) -> RateResult {
    let result = currency::fetch_rates(&job.config, job.cancel.clone()).and_then(|rates| {
        let publication = job
            .publication
            .lock()
            .map_err(|_| "rates publication lock poisoned".to_string())?;
        if *publication != job.generation || job.cancel.load(Ordering::Acquire) {
            return Err("rates refresh cancelled".into());
        }
        Ok(Fetched {
            save_error: currency::save_cache(&job.cache_path, &rates).err(),
            rates,
        })
    });
    RateResult {
        generation: job.generation,
        result,
    }
}
impl RateService {
    pub fn running(&self) -> bool {
        self.active.is_some()
    }
    pub fn ensure_idle(&self) -> Result<(), String> {
        if self.running() {
            Err("exchange rates refresh already running".into())
        } else {
            Ok(())
        }
    }
    pub fn request_refresh(
        &mut self,
        config: CurrencyConfig,
        cache_path: PathBuf,
    ) -> Result<RateJob, String> {
        self.ensure_idle()?;
        self.generation = self.generation.wrapping_add(1);
        *self
            .publication
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = self.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.active = Some((self.generation, cancel.clone()));
        Ok(RateJob {
            generation: self.generation,
            config,
            cache_path,
            cancel,
            publication: self.publication.clone(),
        })
    }
    pub fn complete_startup(
        &mut self,
        job: RateStartupJob,
        result: RateStartupResult,
    ) -> (Option<RateJob>, Option<String>) {
        let problem = match result.cached {
            Ok(Some(rates)) => {
                currency::install(rates);
                None
            }
            Ok(None) => None,
            Err(error) => Some(format!("currency: {error}")),
        };
        let fetch = if job.background_enabled {
            self.request_refresh(job.config, job.cache_path).ok()
        } else {
            None
        };
        (fetch, problem)
    }
    /// Complete exactly once, regardless of the currently open note.
    pub fn complete(&mut self, result: RateResult) -> Option<Result<RateCompletion, String>> {
        let (generation, cancel) = self.active.as_ref()?;
        if result.generation != *generation || cancel.load(Ordering::Acquire) {
            return None;
        }
        self.active = None;
        Some(result.result.map(|fetched| RateCompletion {
            as_of: fetched.rates.as_of.clone(),
            changed: currency::install(fetched.rates),
            save_error: fetched.save_error,
        }))
    }
    pub fn cancel(&mut self) {
        if let Some((generation, cancel)) = self.active.take() {
            let mut publication = self
                .publication
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *publication = generation.wrapping_add(1);
            cancel.store(true, Ordering::Release);
        }
    }
}
impl Drop for RateService {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl crate::NoteSession {
    pub fn invalidate_calc(&mut self) {
        self.calc.stale = true;
        self.calc.range_context = Default::default();
        self.calc.cross_note_refs_generation = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> CurrencyConfig {
        CurrencyConfig {
            argv: vec!["unused-test-runner".into()],
            timeout_seconds: 1,
        }
    }
    fn rates(value: f64, stamp: u64) -> ExchangeRates {
        ExchangeRates {
            base: "EUR".into(),
            rates: [("USD".into(), value)].into_iter().collect(),
            as_of: Some(format!("day-{stamp}")),
            fetched_at: stamp,
        }
    }
    fn result(generation: u64, rates: ExchangeRates) -> RateResult {
        RateResult {
            generation,
            result: Ok(Fetched {
                rates,
                save_error: None,
            }),
        }
    }
    // One test owns all global-rate installations in this crate to avoid racing generations.
    #[test]
    fn global_rate_service_coordinates_startup_switches_failures_duplicates_and_cancellation() {
        let mut service = RateService::default();
        let (job, problem) = service.complete_startup(
            RateStartupJob {
                config: config(),
                cache_path: PathBuf::from("unused"),
                background_enabled: false,
            },
            RateStartupResult {
                cached: Ok(Some(rates(1.125, 1))),
            },
        );
        assert!(job.is_none() && problem.is_none() && !service.running());
        let cached_generation = currency::generation();
        let refresh = service
            .request_refresh(config(), PathBuf::from("unused"))
            .unwrap();
        assert!(service.running());
        assert!(service
            .request_refresh(config(), PathBuf::from("unused"))
            .is_err());
        let doc = crate::Document::from_text("note");
        let history =
            crate::lifecycle::build_history_for_note(doc.lines(), 0, 0, Default::default());
        let mut session = crate::NoteSession::new(history, Default::default(), Default::default());
        session.start_lifetime("a");
        session.start_lifetime("b");
        let completion = service
            .complete(result(refresh.generation, rates(1.25, 2)))
            .unwrap()
            .unwrap();
        assert!(completion.changed);
        assert_eq!(completion.as_of.as_deref(), Some("day-2"));
        assert!(currency::generation() > cached_generation);
        assert!(!service.running());
        assert!(service
            .complete(result(refresh.generation, rates(9.0, 3)))
            .is_none());
        let changed_generation = currency::generation();
        let identical = service
            .request_refresh(config(), PathBuf::from("unused"))
            .unwrap();
        assert!(
            !service
                .complete(result(identical.generation, rates(1.25, 99)))
                .unwrap()
                .unwrap()
                .changed
        );
        assert_eq!(currency::generation(), changed_generation);
        let failing = service
            .request_refresh(config(), PathBuf::from("unused"))
            .unwrap();
        assert!(service
            .complete(RateResult {
                generation: failing.generation,
                result: Err("offline".into())
            })
            .unwrap()
            .is_err());
        assert_eq!(currency::generation(), changed_generation);
        let unchanged = service
            .request_refresh(config(), PathBuf::from("unused"))
            .unwrap();
        assert!(
            !service
                .complete(result(unchanged.generation, rates(1.25, 100)))
                .unwrap()
                .unwrap()
                .changed,
            "failure retained cached values"
        );
        let cancelled = service
            .request_refresh(config(), PathBuf::from("unused"))
            .unwrap();
        service.cancel();
        assert!(cancelled.cancel.load(Ordering::Acquire));
        let newer = service
            .request_refresh(config(), PathBuf::from("unused"))
            .unwrap();
        assert_ne!(cancelled.generation, newer.generation);
        assert!(service
            .complete(result(cancelled.generation, rates(8.0, 101)))
            .is_none());
        assert!(
            service.running(),
            "late old completion cannot consume newer refresh"
        );
        assert!(
            !service
                .complete(result(newer.generation, rates(1.25, 102)))
                .unwrap()
                .unwrap()
                .changed
        );
        let (fetch, problem) = service.complete_startup(
            RateStartupJob {
                config: config(),
                cache_path: PathBuf::from("unused"),
                background_enabled: true,
            },
            RateStartupResult {
                cached: Err("invalid cache".into()),
            },
        );
        assert!(fetch.is_some());
        assert_eq!(problem.as_deref(), Some("currency: invalid cache"));
        assert_eq!(currency::generation(), changed_generation);
        service.cancel();
    }
    #[test]
    fn rate_messages_are_owned_executor_inputs_and_outputs() {
        fn require<T: Send + 'static>() {}
        require::<RateJob>();
        require::<RateResult>();
        require::<RateStartupJob>();
        require::<RateStartupResult>();
    }
}
