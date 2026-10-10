use super::super::currency::{CurrencyState, Fetched};
use super::*;
use app_core::currency::ExchangeRates;

// The only TUI test that installs rates, which are process-wide.
#[test]
fn refreshed_rates_update_calc_results_and_failures_keep_them() {
    let (db, mut app, path) = app_with_note("10 USD to EUR");
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
    // A dependency evaluated with the old rates must be loaded again.
    let dep = ulid::Ulid::new().to_string();
    db.save_note(&dep, "price := 20 EUR to USD").unwrap();
    app_core::cross_note::load_note_exports(&db, &app.calc.engine, &app.cross_note_var_index, &dep);
    app.editor.lines = vec![format!("[[{dep}]].price * 2")];
    app.recompute_calc_whole_note();
    assert_eq!(app.calc.results[0].as_deref(), Some("50"));
    app.currency = CurrencyState::with_result(Ok(Fetched {
        rates: ExchangeRates {
            base: "EUR".into(),
            rates: [("USD".to_string(), 2.0)].into_iter().collect(),
            as_of: None,
            fetched_at: 0,
        },
        save_error: None,
    }));
    app.poll_currency_refresh();
    assert_eq!(app.calc.results[0].as_deref(), Some("80"));

    // Host dispatch refuses a second worker while one is pending.
    app.currency = CurrencyState::with_result(Err("offline".into()));
    app.execute_terminal_command(&db, "currency refresh");
    assert_eq!(app.status, "exchange rates refresh already running");
    #[cfg(unix)]
    {
        let dir =
            std::env::temp_dir().join(format!("slate-currency-startup-{}", ulid::Ulid::new()));
        fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("exchange_rates.json");
        app_core::currency::save_cache(
            &cache,
            &ExchangeRates {
                base: "EUR".into(),
                rates: [("USD".to_string(), 2.0)].into_iter().collect(),
                as_of: None,
                fetched_at: app_core::currency::now_unix_seconds(),
            },
        )
        .unwrap();
        let config = app_core::currency::CurrencyConfig {
            argv: vec![
                "sh".into(),
                "-c".into(),
                r#"printf '{"base":"EUR","rates":{"USD":4}}'"#.into(),
            ],
            refresh_hours: 12,
            timeout_seconds: 5,
        };
        // Disabling startup work loads the cache without running the script.
        let (state, problem) =
            CurrencyState::start_with_config(config.clone(), cache.clone(), false);
        assert!(problem.is_none());
        app.currency = state;
        app.status = "unchanged".into();
        app.poll_currency_refresh();
        assert_eq!(app.status, "unchanged");
        assert_eq!(
            app_core::currency::load_cache(&cache)
                .unwrap()
                .unwrap()
                .rates["USD"],
            2.0
        );

        // A fresh cache still permits exactly one startup fetch.
        let (state, problem) = CurrencyState::start_with_config(config, cache.clone(), true);
        assert!(problem.is_none());
        app.currency = state;
        app.execute_terminal_command(&db, ":currentcy refresh");
        assert_eq!(app.status, "exchange rates refresh already running");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !app.status.starts_with("exchange rates updated") {
            assert!(std::time::Instant::now() < deadline, "{}", app.status);
            app.poll_currency_refresh();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(app.calc.results[0].as_deref(), Some("160"));
        assert_eq!(
            app_core::currency::load_cache(&cache)
                .unwrap()
                .unwrap()
                .rates["USD"],
            4.0
        );
        fs::remove_dir_all(dir).unwrap();
    }
    let _ = fs::remove_file(path);
}
