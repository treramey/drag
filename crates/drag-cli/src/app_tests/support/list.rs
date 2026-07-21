use super::*;

#[tokio::test]
async fn dropping_a_pending_task_aborts_its_join_handle() {
    let handle = tokio::spawn(std::future::pending::<()>());
    let abort_handle = handle.abort_handle();
    let task = AbortOnDropTask::new(handle);
    tokio::task::yield_now().await;

    drop(task);

    let cancelled = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !abort_handle.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(cancelled.is_ok());
}

#[tokio::test]
async fn list_fetch_debounce_does_not_poll_the_load_until_the_quiet_period_ends() {
    let polled = Arc::new(AtomicBool::new(false));
    let load_polled = Arc::clone(&polled);
    let load = std::future::poll_fn(move |_| {
        load_polled.store(true, Ordering::SeqCst);
        std::task::Poll::Ready(())
    });
    let (release, quiet_period) = tokio::sync::oneshot::channel::<()>();
    let quiet_period = async move {
        let _ = quiet_period.await;
    };
    let debounced = debounce_list_fetch(quiet_period, load);
    tokio::pin!(debounced);

    tokio::select! {
        () = &mut debounced => panic!("load completed before the quiet period"),
        () = tokio::task::yield_now() => {}
    }
    assert!(!polled.load(Ordering::SeqCst));

    assert!(release.send(()).is_ok());
    debounced.await;
    assert!(polled.load(Ordering::SeqCst));
}

#[test]
fn continuation_report_is_retained_only_as_suspense_background() {
    let report = empty_list_report(false);
    let date = report.selected_date();
    let mut reports = BTreeMap::from([(
        date,
        CachedListReport {
            report,
            reusable: false,
        },
    )]);

    let cached = take_reusable_report(&mut reports, date);

    assert!(cached.is_none() && reports.contains_key(&date));
}

#[tokio::test]
async fn eligible_human_list_is_presented_by_the_injected_report_session() -> Result<(), CliError> {
    let temp = TempDir::new()?;
    let selected_dates = Arc::new(Mutex::new(Vec::new()));
    let app = App::with_connection_verifier(
        temp.path().join("config.json"),
        FakeVerifier {
            jira_error: None,
            tempo_error: None,
            tempo_accounts: Arc::new(Mutex::new(Vec::new())),
            config_update: None,
        },
    )
    .with_list_report_session(FakeListReportSession {
        eligible: true,
        selected_dates: Arc::clone(&selected_dates),
    });

    let rendered = app.finish_list(empty_list_report(false), true).await?;

    assert!(rendered.is_none());
    let dates = selected_dates
        .lock()
        .map_err(|_| CliError::Io(std::io::Error::other("selected dates lock poisoned")))?;
    assert_eq!(
        *dates,
        [NaiveDate::from_ymd_opt(2026, 7, 14).unwrap_or(NaiveDate::MIN)]
    );
    Ok(())
}

#[tokio::test]
async fn explicit_json_or_ineligible_human_list_remains_non_interactive() -> Result<(), CliError> {
    for (interactive, eligible) in [(false, true), (true, false)] {
        let temp = TempDir::new()?;
        let selected_dates = Arc::new(Mutex::new(Vec::new()));
        let app = App::with_connection_verifier(
            temp.path().join("config.json"),
            FakeVerifier {
                jira_error: None,
                tempo_error: None,
                tempo_accounts: Arc::new(Mutex::new(Vec::new())),
                config_update: None,
            },
        )
        .with_list_report_session(FakeListReportSession {
            eligible,
            selected_dates: Arc::clone(&selected_dates),
        });

        let rendered = app
            .finish_list(empty_list_report(false), interactive)
            .await?;

        let rendered = rendered.ok_or_else(|| CliError::Api("missing plain result".to_owned()))?;
        assert_eq!(rendered.data["date"], "2026-07-14");
        assert!(selected_dates
            .lock()
            .map_err(|_| CliError::Io(std::io::Error::other("selected dates lock poisoned")))?
            .is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn eligible_verbose_human_list_is_presented_by_the_report_session() -> Result<(), CliError> {
    let temp = TempDir::new()?;
    let selected_dates = Arc::new(Mutex::new(Vec::new()));
    let app = App::with_connection_verifier(
        temp.path().join("config.json"),
        FakeVerifier {
            jira_error: None,
            tempo_error: None,
            tempo_accounts: Arc::new(Mutex::new(Vec::new())),
            config_update: None,
        },
    )
    .with_list_report_session(FakeListReportSession {
        eligible: true,
        selected_dates: Arc::clone(&selected_dates),
    });

    let rendered = app.finish_list(empty_list_report(true), true).await?;

    assert!(rendered.is_none());
    assert_eq!(
        *selected_dates
            .lock()
            .map_err(|_| CliError::Io(std::io::Error::other("selected dates lock poisoned")))?,
        [NaiveDate::from_ymd_opt(2026, 7, 14).unwrap_or(NaiveDate::MIN)]
    );
    Ok(())
}
