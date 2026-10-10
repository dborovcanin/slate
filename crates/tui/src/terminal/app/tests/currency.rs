use super::super::currency::{CurrencyState, Fetched};
use super::*;
use app_core::currency::ExchangeRates;

// The only TUI test that installs rates, which are process-wide.
#[test]
fn refreshed_rates_update_calc_results_and_failures_keep_them() {
    let (_db, mut app, path) = app_with_note("10 USD to EUR");
    app.currency = CurrencyState::with_result(Ok(Fetched {
        rates: ExchangeRates {
            base: "EUR".into(),
            rates: [("USD".to_string(), 1.25)].into_iter().collect(),
            as_of: Some("2026-10-09".into()),
            fetched_at: 0,
        },
        save_error: None,
    }));
    app.poll_currency_refresh();
    assert_eq!(app.calc.results[0].as_deref(), Some("8 EUR"));
    assert_eq!(app.status, "exchange rates updated (2026-10-09)");

    app.currency = CurrencyState::with_result(Err("offline".into()));
    app.poll_currency_refresh();
    assert_eq!(app.status, "exchange rates: offline");
    assert_eq!(app.calc.results[0].as_deref(), Some("8 EUR"));
    let _ = fs::remove_file(path);
}
