use super::*;

impl CortexService {
    pub async fn list_apps(&self, req: ListAppsRequest) -> ServiceResult<ListAppsResponse> {
        let from = parse_optional_timestamp(req.since.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let result = self
            .run_db("list_apps", move |pool| {
                db::list_apps(
                    pool,
                    &db::ListAppsParams {
                        hostname: req.host.as_deref(),
                        from: from.as_deref(),
                        to: to.as_deref(),
                        limit: req.limit.unwrap_or(500) as usize,
                        offset: req.offset.unwrap_or(0) as usize,
                    },
                )
            })
            .await?;
        Ok(ListAppsResponse {
            apps: result.apps.into_iter().map(Into::into).collect(),
            total: result.total,
        })
    }

    pub async fn list_source_ips(
        &self,
        req: ListSourceIpsRequest,
    ) -> ServiceResult<ListSourceIpsResponse> {
        let result = self
            .run_db("list_source_ips", move |pool| {
                db::list_source_ips(
                    pool,
                    &db::ListSourceIpsParams {
                        limit: req.limit.unwrap_or(500) as usize,
                        offset: req.offset.unwrap_or(0) as usize,
                    },
                )
            })
            .await?;
        Ok(ListSourceIpsResponse {
            source_ips: result.source_ips.into_iter().map(Into::into).collect(),
            total: result.total,
        })
    }

    pub async fn timeline(&self, req: TimelineRequest) -> ServiceResult<TimelineResponse> {
        let bucket_str = req.bucket.unwrap_or_else(|| "hour".into());
        let bucket = Bucket::parse(&bucket_str).ok_or_else(|| {
            ServiceError::InvalidInput(format!(
                "Invalid bucket '{bucket_str}'. Expected: minute, hour, day, week, month"
            ))
        })?;
        let group_by = match req.group_by.as_deref() {
            None => TimelineGroupBy::None,
            Some(g) => TimelineGroupBy::parse(g).ok_or_else(|| {
                ServiceError::InvalidInput(format!(
                    "Invalid group_by '{g}'. Expected: hostname, severity, app_name"
                ))
            })?,
        };
        // Centralized default lookback (bead dyqw): apply the bucket-sized window
        // ONLY when neither `from` nor `to` is supplied. This prevents the full
        // table scan on an unbounded query while preserving the zl9y guard — if
        // `to` is set but `from` is omitted we must NOT inject a default `from`,
        // or we'd create an impossible range. All transport call sites (api.rs,
        // mcp/tools.rs, cli/dispatch_surface.rs) now pass `from`/`to` through
        // verbatim, so this is the single source of truth.
        let from_raw = match (req.since, req.until.is_some()) {
            (None, false) => chrono::Utc::now()
                .checked_sub_signed(chrono::Duration::days(bucket.default_lookback_days()))
                .map(|dt| dt.to_rfc3339()),
            (other, _) => other,
        };
        let from = parse_optional_timestamp(from_raw.as_deref(), "since")?;
        let to = parse_optional_timestamp(req.until.as_deref(), "until")?;
        let severity_in = match req.severity_min.as_deref() {
            Some(min) => Some(severity_at_or_above(min)?),
            None => None,
        };
        let group_by_label = req.group_by.clone();
        // Rollup-served buckets carry the rollup's last-refresh time as their
        // `as_of` staleness marker; the live `minute` bucket is always current.
        let served_by_rollup = bucket.served_by_hourly_rollup();
        let (points, rollup_as_of) = self
            .run_db("timeline", move |pool| {
                let points = db::timeline(
                    pool,
                    bucket,
                    group_by,
                    from.as_deref(),
                    to.as_deref(),
                    req.host.as_deref(),
                    req.app.as_deref(),
                    severity_in.as_deref(),
                )?;
                let as_of = if served_by_rollup {
                    db::timeline_rollup_status(pool)?.refreshed_at
                } else {
                    None
                };
                Ok((points, as_of))
            })
            .await?;
        Ok(TimelineResponse {
            bucket: bucket_str,
            group_by: group_by_label,
            points: points.into_iter().map(Into::into).collect(),
            rollup_as_of,
        })
    }

    pub async fn patterns(&self, req: PatternsRequest) -> ServiceResult<PatternsResponse> {
        let until_dt = req
            .until
            .as_deref()
            .map(|until| parse_required_timestamp(until, "until"))
            .transpose()?;
        let to = until_dt.map(rfc3339_z);
        // Apply the advertised 24h default when the caller omits `since`. Anchor
        // the window to `until` (until - 24h) when one is provided so an
        // `--until`-only request stays bounded instead of scanning the newest
        // PATTERN_SCAN_LIMIT rows across all retained history; otherwise anchor
        // to now.
        let from = match req.since.as_deref() {
            Some(since) => Some(rfc3339_z(parse_required_timestamp(since, "since")?)),
            None => {
                let anchor = until_dt.unwrap_or_else(chrono::Utc::now);
                Some(rfc3339_z(anchor - chrono::Duration::hours(24)))
            }
        };
        let severity_in = match req.severity_min.as_deref() {
            Some(min) => Some(severity_at_or_above(min)?),
            None => None,
        };
        let scan_limit = req.scan_limit.unwrap_or(db::PATTERN_SCAN_LIMIT_MAX);
        let top_n = req.top_n.unwrap_or(20).min(200);
        let (patterns, scanned, truncated) = self
            .run_heavy_db("patterns", move |pool| {
                let (rows, truncated) = db::fetch_pattern_rows(
                    pool,
                    from.as_deref(),
                    to.as_deref(),
                    req.host.as_deref(),
                    req.app.as_deref(),
                    severity_in.as_deref(),
                    scan_limit,
                )?;
                let (patterns, scanned) = db::cluster_pattern_rows(rows, top_n);
                Ok((patterns, scanned, truncated))
            })
            .await?;
        Ok(PatternsResponse {
            patterns: patterns.into_iter().map(Into::into).collect(),
            scanned,
            truncated,
        })
    }

    pub async fn context(&self, req: ContextRequest) -> ServiceResult<ContextResponse> {
        let before = req.before.unwrap_or(10).min(500);
        let after = req.after.unwrap_or(10).min(500);

        // When the caller anchors via hostname+timestamp, parse and re-emit the
        // timestamp through the same path used elsewhere in the service so
        // `2026-01-01T00:00:00Z` and `2026-01-01T00:00:00+00:00` (and offset
        // forms like `+02:00`) compare correctly against stored RFC3339 values
        // — TEXT comparisons in SQLite would otherwise treat them as different.
        let synthetic_timestamp = if req.log_id.is_none() {
            match req.timestamp.as_deref() {
                Some(raw) => Some(rfc3339_z(parse_required_timestamp(raw, "timestamp")?)),
                None => None,
            }
        } else {
            None
        };

        let resolved = self
            .run_db("context", move |pool| -> anyhow::Result<_> {
                let (reference, hostname, timestamp, id): (LogEntry, String, String, Option<i64>) =
                    if let Some(id) = req.log_id {
                        let mut row = db::fetch_log_by_id(pool, id)?
                            .ok_or_else(|| anyhow::anyhow!("context_log_not_found:{id}"))?;
                        display_device_hostname(&mut row);
                        let entry = LogEntry {
                            id: row.id,
                            timestamp: row.timestamp.clone(),
                            hostname: row.hostname.clone(),
                            facility: row.facility.clone(),
                            severity: row.severity.clone(),
                            app_name: row.app_name.clone(),
                            process_id: row.process_id.clone(),
                            message: row.message.clone(),
                            received_at: row.received_at.clone(),
                            source_ip: row.source_ip.clone(),
                            ai_tool: row.ai_tool.clone(),
                            ai_project: row.ai_project.clone(),
                            ai_session_id: row.ai_session_id.clone(),
                            ai_transcript_path: row.ai_transcript_path.clone(),
                            metadata_json: row.metadata_json.clone(),
                        };
                        (entry, row.hostname, row.timestamp, Some(row.id))
                    } else {
                        let hostname = req
                            .host
                            .clone()
                            .ok_or_else(|| anyhow::anyhow!("context_missing_pivot"))?;
                        let timestamp = synthetic_timestamp
                            .ok_or_else(|| anyhow::anyhow!("context_missing_pivot"))?;
                        let synthetic = LogEntry {
                            id: 0,
                            timestamp: timestamp.clone(),
                            hostname: hostname.clone(),
                            facility: None,
                            severity: String::new(),
                            app_name: None,
                            process_id: None,
                            message: "<reference timestamp>".into(),
                            received_at: timestamp.clone(),
                            source_ip: String::new(),
                            ai_tool: None,
                            ai_project: None,
                            ai_session_id: None,
                            ai_transcript_path: None,
                            metadata_json: None,
                        };
                        (synthetic, hostname, timestamp, None)
                    };

                let (before_rows, after_rows) = db::context_around(
                    pool,
                    &ContextRef {
                        id,
                        hostname,
                        timestamp,
                    },
                    before,
                    after,
                )?;
                Ok((reference, before_rows, after_rows))
            })
            .await
            .map_err(|error| match error {
                ServiceError::Internal(inner) => {
                    let msg = inner.to_string();
                    if msg == "context_missing_pivot" {
                        ServiceError::InvalidInput(
                            "Either `log_id` or both `host` + `timestamp` are required".into(),
                        )
                    } else if let Some(id) = msg.strip_prefix("context_log_not_found:") {
                        ServiceError::NotFound(format!("No log found for id {id}"))
                    } else {
                        ServiceError::Internal(inner)
                    }
                }
                other => other,
            })?;
        let (reference, before_rows, after_rows) = resolved;
        Ok(ContextResponse {
            reference,
            before: before_rows.into_iter().map(Into::into).collect(),
            after: after_rows.into_iter().map(Into::into).collect(),
        })
    }

    pub async fn get_log(&self, req: GetLogRequest) -> ServiceResult<GetLogResponse> {
        let id = req.id;
        let mut row = self
            .run_db("get_log", move |pool| db::fetch_log_by_id(pool, id))
            .await?
            .ok_or_else(|| ServiceError::InvalidInput(format!("No log found for id {id}")))?;
        display_device_hostname(&mut row);
        Ok(GetLogResponse { log: row.into() })
    }

    pub async fn ingest_rate(&self, req: IngestRateRequest) -> ServiceResult<IngestRateResponse> {
        let now_dt = Utc::now();
        let now = rfc3339_z(now_dt);
        let cut_1m = rfc3339_z(now_dt - chrono::Duration::seconds(60));
        let cut_5m = rfc3339_z(now_dt - chrono::Duration::seconds(300));
        let cut_15m = rfc3339_z(now_dt - chrono::Duration::seconds(900));
        let want_by_host = req.by_host.unwrap_or(false);

        let storage = self.storage.clone();
        let now_clone = now.clone();
        let cut_1m_q = cut_1m.clone();
        let cut_5m_q = cut_5m.clone();
        let cut_15m_q = cut_15m.clone();
        let result = self
            .run_heavy_db("ingest_rate", move |pool| -> anyhow::Result<_> {
                let buckets = db::ingest_rate(pool, &now_clone, &cut_1m_q, &cut_5m_q, &cut_15m_q)?;
                let by_host = if want_by_host {
                    Some(db::ingest_rate_by_host(
                        pool, &now_clone, &cut_1m_q, &cut_5m_q, &cut_15m_q,
                    )?)
                } else {
                    None
                };
                let metrics = db::get_storage_metrics(pool, &storage)?;
                let write_blocked = db::exceeds_trigger(&metrics, &storage);
                Ok((buckets, by_host, write_blocked))
            })
            .await?;
        let (buckets, by_host, write_blocked) = result;
        Ok(IngestRateResponse {
            now,
            buckets: buckets.into(),
            write_blocked,
            by_host: by_host.map(|rows| rows.into_iter().map(Into::into).collect()),
        })
    }

    pub async fn ingest_source_kind_health(
        &self,
    ) -> ServiceResult<Vec<db::IngestSourceKindHealth>> {
        let now_dt = Utc::now();
        let now = rfc3339_z(now_dt);
        let cut_15m = rfc3339_z(now_dt - chrono::Duration::minutes(15));
        let cut_1h = rfc3339_z(now_dt - chrono::Duration::hours(1));
        let cut_24h = rfc3339_z(now_dt - chrono::Duration::hours(24));

        self.run_db("ingest_source_kind_health", move |pool| {
            db::ingest_source_kind_health(pool, &now, &cut_15m, &cut_1h, &cut_24h)
        })
        .await
    }

    pub async fn silent_hosts(
        &self,
        req: SilentHostsRequest,
    ) -> ServiceResult<SilentHostsResponse> {
        let silent_minutes = req.silent_minutes.unwrap_or(30).min(60 * 24 * 7);
        let now_dt = Utc::now();
        let now = rfc3339_z(now_dt);
        let cutoff_dt = now_dt - chrono::Duration::minutes(i64::from(silent_minutes));
        let cutoff = rfc3339_z(cutoff_dt);
        let now_unix = now_dt.timestamp();
        let cutoff_q = cutoff.clone();
        let hosts = self
            .run_db("silent_hosts", move |pool| {
                db::silent_hosts(pool, &cutoff_q, now_unix)
            })
            .await?;
        Ok(SilentHostsResponse {
            silent_minutes,
            cutoff,
            now,
            hosts: hosts.into_iter().map(Into::into).collect(),
        })
    }

    pub async fn clock_skew(&self, req: ClockSkewRequest) -> ServiceResult<ClockSkewResponse> {
        // Compared against `received_at`, which SQLite stores in `Z` form, so
        // emit the canonical Z-form regardless of input shape.
        let since_str = match req.since {
            Some(s) => rfc3339_z(parse_required_timestamp(&s, "since")?),
            None => rfc3339_z(Utc::now() - chrono::Duration::hours(24)),
        };
        let q = since_str.clone();
        let limit = req.limit.map(|limit| limit.clamp(1, 100));
        let hosts = self
            .run_heavy_db("clock_skew", move |pool| db::clock_skew(pool, &q, limit))
            .await?;
        Ok(ClockSkewResponse {
            since: since_str,
            hosts: hosts.into_iter().map(Into::into).collect(),
        })
    }

    pub async fn anomalies(&self, req: AnomaliesRequest) -> ServiceResult<AnomaliesResponse> {
        let recent_minutes = req.recent_minutes.unwrap_or(15).clamp(1, 60 * 24);
        let baseline_minutes = req.baseline_minutes.unwrap_or(360).clamp(1, 60 * 24 * 7);
        let now_dt = chrono::Utc::now();
        let recent_to = rfc3339_z(now_dt);
        let recent_from_dt = now_dt - chrono::Duration::minutes(i64::from(recent_minutes));
        let recent_from = rfc3339_z(recent_from_dt);
        let baseline_to = recent_from.clone();
        let baseline_from =
            rfc3339_z(recent_from_dt - chrono::Duration::minutes(i64::from(baseline_minutes)));

        let rf = recent_from.clone();
        let rt = recent_to.clone();
        let bf = baseline_from.clone();
        let bt = baseline_to.clone();
        let hosts = self
            .run_heavy_db("anomalies", move |pool| {
                db::anomalies(pool, &rf, &rt, &bf, &bt, recent_minutes, baseline_minutes)
            })
            .await?;
        Ok(AnomaliesResponse {
            recent_from,
            recent_to,
            baseline_from,
            baseline_to,
            recent_minutes,
            baseline_minutes,
            hosts: hosts.into_iter().map(Into::into).collect(),
        })
    }

    pub async fn compare(&self, req: CompareRequest) -> ServiceResult<CompareResponse> {
        let supplied = [
            req.a_from.as_ref(),
            req.a_to.as_ref(),
            req.b_from.as_ref(),
            req.b_to.as_ref(),
        ];
        let (a_from_ts, a_to_ts, b_from_ts, b_to_ts) = if supplied.iter().all(|v| v.is_none()) {
            let b_to = chrono::Utc::now();
            let b_from = b_to - chrono::Duration::hours(1);
            let a_to = b_from;
            let a_from = a_to - chrono::Duration::hours(1);
            (a_from, a_to, b_from, b_to)
        } else if supplied.iter().all(|v| v.is_some()) {
            (
                parse_required_timestamp(req.a_from.as_deref().unwrap(), "a_from")?,
                parse_required_timestamp(req.a_to.as_deref().unwrap(), "a_to")?,
                parse_required_timestamp(req.b_from.as_deref().unwrap(), "b_from")?,
                parse_required_timestamp(req.b_to.as_deref().unwrap(), "b_to")?,
            )
        } else {
            return Err(ServiceError::InvalidInput(
                "compare accepts either no timestamps or all of a_from, a_to, b_from, and b_to"
                    .into(),
            ));
        };

        // Each range scans the timestamp partition; an uncapped width let a
        // single compare call scan the whole table on retention-disabled DBs
        // (full-review PM4). 92 days covers a quarter and exceeds the default
        // 90-day retention window.
        const MAX_COMPARE_RANGE_DAYS: i64 = 92;
        for (label, from, to) in [("a", a_from_ts, a_to_ts), ("b", b_from_ts, b_to_ts)] {
            if to < from {
                return Err(crate::app::ServiceError::InvalidInput(format!(
                    "{label}_to must not be earlier than {label}_from"
                )));
            }
            if to - from > chrono::Duration::days(MAX_COMPARE_RANGE_DAYS) {
                return Err(crate::app::ServiceError::InvalidInput(format!(
                    "range {label} is wider than {MAX_COMPARE_RANGE_DAYS} days; \
                     narrow the window"
                )));
            }
        }

        let a_from = rfc3339_z(a_from_ts);
        let a_to = rfc3339_z(a_to_ts);
        let b_from = rfc3339_z(b_from_ts);
        let b_to = rfc3339_z(b_to_ts);

        let a_from_q = a_from.clone();
        let a_to_q = a_to.clone();
        let b_from_q = b_from.clone();
        let b_to_q = b_to.clone();
        let result = self
            .run_heavy_db("compare", move |pool| -> anyhow::Result<_> {
                let a = db::summarize_range(pool, &a_from_q, &a_to_q)?;
                let b = db::summarize_range(pool, &b_from_q, &b_to_q)?;
                Ok((a, b))
            })
            .await?;
        let (a, b) = result;
        let delta_total_logs = b.total_logs - a.total_logs;
        let delta_total_errors = b.total_errors - a.total_errors;
        Ok(CompareResponse {
            a: a.into(),
            b: b.into(),
            delta_total_logs,
            delta_total_errors,
        })
    }
}

fn display_device_hostname(row: &mut db::LogEntryWithRaw) {
    if let Some(hostname) = crate::forwarded_host::subject_hostname(
        &row.hostname,
        &row.source_ip,
        row.metadata_json.as_deref(),
    ) {
        row.hostname = hostname;
    }
}
