pub const DAY_MS: i64 = 86_400_000;
const MIN_RATE_BYTES_PER_DAY: i64 = 10_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapacityPoint {
    pub captured_at_ms: i64,
    pub total_bytes: i64,
    pub free_bytes: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    Early,
    Developing,
    Reliable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageForecast {
    pub confidence: Confidence,
    pub days_low: i64,
    pub days_high: i64,
    pub bytes_per_day: i64,
    pub observations: usize,
    pub span_days: i64,
    pub latest_free_bytes: i64,
    pub threshold_bytes: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForecastState {
    NeedHistory {
        observations: usize,
        span_days: i64,
    },
    AlreadyLow {
        free_bytes: i64,
        threshold_bytes: i64,
    },
    Flat {
        observations: usize,
        span_days: i64,
    },
    Improving {
        bytes_per_day: i64,
        observations: usize,
        span_days: i64,
    },
    Volatile {
        observations: usize,
        span_days: i64,
    },
    Estimate(StorageForecast),
}

fn median(values: &mut [i64]) -> i64 {
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        values[middle]
    } else {
        ((values[middle - 1] as i128 + values[middle] as i128) / 2) as i64
    }
}

fn confidence(observations: usize, span_days: i64) -> Option<Confidence> {
    if observations >= 8 && span_days >= 30 {
        Some(Confidence::Reliable)
    } else if observations >= 5 && span_days >= 14 {
        Some(Confidence::Developing)
    } else if observations >= 3 && span_days >= 7 {
        Some(Confidence::Early)
    } else {
        None
    }
}

fn clamp_i128(value: i128) -> i64 {
    value.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

fn ceil_days(bytes: i64, bytes_per_day: i64) -> i64 {
    if bytes <= 0 {
        return 0;
    }
    let numerator = bytes as i128 + bytes_per_day.max(1) as i128 - 1;
    (numerator / bytes_per_day.max(1) as i128).clamp(1, i64::MAX as i128) as i64
}

pub fn analyze(points: &[CapacityPoint], threshold_bytes: i64) -> ForecastState {
    let valid: Vec<CapacityPoint> = points
        .iter()
        .copied()
        .filter(|point| {
            point.captured_at_ms >= 0
                && point.total_bytes > 0
                && point.free_bytes >= 0
                && point.free_bytes <= point.total_bytes
        })
        .collect();
    let Some(latest_total) = valid
        .iter()
        .max_by_key(|point| point.captured_at_ms)
        .map(|point| point.total_bytes)
    else {
        return ForecastState::NeedHistory {
            observations: 0,
            span_days: 0,
        };
    };

    let mut by_time = std::collections::BTreeMap::new();
    for point in valid
        .into_iter()
        .filter(|point| point.total_bytes == latest_total)
    {
        by_time.insert(point.captured_at_ms, point);
    }
    let points: Vec<CapacityPoint> = by_time.into_values().collect();
    let observations = points.len();
    let span_days = points
        .first()
        .zip(points.last())
        .map(|(first, last)| (last.captured_at_ms - first.captured_at_ms) / DAY_MS)
        .unwrap_or(0)
        .max(0);
    let Some(confidence) = confidence(observations, span_days) else {
        return ForecastState::NeedHistory {
            observations,
            span_days,
        };
    };
    let latest_free_bytes = points.last().map(|point| point.free_bytes).unwrap_or(0);
    let threshold_bytes = threshold_bytes.max(0);
    if latest_free_bytes <= threshold_bytes {
        return ForecastState::AlreadyLow {
            free_bytes: latest_free_bytes,
            threshold_bytes,
        };
    }

    let mut rates: Vec<i64> = points
        .windows(2)
        .filter_map(|pair| {
            let elapsed = pair[1]
                .captured_at_ms
                .saturating_sub(pair[0].captured_at_ms);
            if elapsed <= 0 {
                return None;
            }
            Some(clamp_i128(
                (pair[0].free_bytes as i128 - pair[1].free_bytes as i128) * DAY_MS as i128
                    / elapsed as i128,
            ))
        })
        .collect();
    let bytes_per_day = median(&mut rates);
    let positive_intervals = rates.iter().filter(|rate| **rate > 0).count();
    let negative_intervals = rates.iter().filter(|rate| **rate < 0).count();
    let balanced_signs =
        positive_intervals > 0 && negative_intervals > 0 && positive_intervals * 2 <= rates.len();
    if balanced_signs {
        return ForecastState::Volatile {
            observations,
            span_days,
        };
    }
    if bytes_per_day.abs() < MIN_RATE_BYTES_PER_DAY {
        return ForecastState::Flat {
            observations,
            span_days,
        };
    }
    if bytes_per_day < 0 {
        return ForecastState::Improving {
            bytes_per_day,
            observations,
            span_days,
        };
    }

    let mut deviations: Vec<i64> = rates
        .iter()
        .map(|rate| clamp_i128((*rate as i128 - bytes_per_day as i128).abs()))
        .collect();
    let median_deviation = median(&mut deviations);
    if median_deviation > bytes_per_day.saturating_mul(2) {
        return ForecastState::Volatile {
            observations,
            span_days,
        };
    }

    let floor_percent = match confidence {
        Confidence::Early => 35,
        Confidence::Developing => 25,
        Confidence::Reliable => 15,
    };
    let floor_uncertainty = clamp_i128(bytes_per_day as i128 * floor_percent / 100);
    let uncertainty = median_deviation.max(floor_uncertainty);
    let fast_rate = bytes_per_day.saturating_add(uncertainty).max(1);
    let slow_rate = bytes_per_day
        .saturating_sub(uncertainty)
        .max(bytes_per_day / 5)
        .max(1);
    let remaining = latest_free_bytes.saturating_sub(threshold_bytes).max(0);
    let central_days = ceil_days(remaining, bytes_per_day);
    let days_low = ceil_days(remaining, fast_rate).max(1);
    let days_high = ceil_days(remaining, slow_rate)
        .min(central_days.saturating_mul(5))
        .max(days_low);

    ForecastState::Estimate(StorageForecast {
        confidence,
        days_low,
        days_high,
        bytes_per_day,
        observations,
        span_days,
        latest_free_bytes,
        threshold_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: i64 = 1_000_000_000;

    fn point(day: i64, free_gb: i64) -> CapacityPoint {
        CapacityPoint {
            captured_at_ms: day * DAY_MS,
            total_bytes: 250 * GB,
            free_bytes: free_gb * GB,
        }
    }

    #[test]
    fn needs_three_points_and_seven_days() {
        assert_eq!(
            analyze(&[point(0, 80), point(7, 70)], 15 * GB),
            ForecastState::NeedHistory {
                observations: 2,
                span_days: 7,
            }
        );
        assert_eq!(
            analyze(&[point(0, 80), point(3, 76), point(6, 72)], 15 * GB),
            ForecastState::NeedHistory {
                observations: 3,
                span_days: 6,
            }
        );
    }

    #[test]
    fn confidence_follows_exact_evidence_thresholds() {
        let early = analyze(&[point(0, 80), point(3, 77), point(7, 73)], 15 * GB);
        assert!(matches!(
            early,
            ForecastState::Estimate(StorageForecast {
                confidence: Confidence::Early,
                ..
            })
        ));

        let developing = analyze(
            &[
                point(0, 90),
                point(4, 86),
                point(8, 82),
                point(11, 79),
                point(14, 76),
            ],
            15 * GB,
        );
        assert!(matches!(
            developing,
            ForecastState::Estimate(StorageForecast {
                confidence: Confidence::Developing,
                ..
            })
        ));

        let reliable = analyze(
            &[
                point(0, 100),
                point(5, 95),
                point(10, 90),
                point(15, 85),
                point(20, 80),
                point(24, 76),
                point(27, 73),
                point(30, 70),
            ],
            15 * GB,
        );
        assert!(matches!(
            reliable,
            ForecastState::Estimate(StorageForecast {
                confidence: Confidence::Reliable,
                ..
            })
        ));
    }

    #[test]
    fn median_rate_ignores_one_cleanup_spike() {
        let state = analyze(
            &[
                point(0, 80),
                point(2, 76),
                point(4, 72),
                point(6, 90),
                point(8, 86),
            ],
            15 * GB,
        );
        let ForecastState::Estimate(forecast) = state else {
            panic!("expected estimate")
        };
        assert_eq!(forecast.bytes_per_day, 2 * GB);
        assert!(forecast.days_low <= forecast.days_high);
    }

    #[test]
    fn flat_improving_volatile_and_already_low_are_not_estimates() {
        assert!(matches!(
            analyze(&[point(0, 80), point(4, 80), point(8, 80)], 15 * GB),
            ForecastState::Flat { .. }
        ));
        assert!(matches!(
            analyze(&[point(0, 70), point(4, 75), point(8, 80)], 15 * GB),
            ForecastState::Improving { .. }
        ));
        assert!(matches!(
            analyze(
                &[
                    point(0, 80),
                    point(2, 70),
                    point(4, 82),
                    point(6, 70),
                    point(8, 82),
                ],
                15 * GB,
            ),
            ForecastState::Volatile { .. }
        ));
        assert!(matches!(
            analyze(&[point(0, 20), point(4, 16), point(8, 14)], 15 * GB),
            ForecastState::AlreadyLow { .. }
        ));
    }

    #[test]
    fn invalid_duplicate_and_different_capacity_points_are_excluded() {
        let mut wrong_capacity = point(2, 78);
        wrong_capacity.total_bytes = 500 * GB;
        let impossible = CapacityPoint {
            captured_at_ms: 3 * DAY_MS,
            total_bytes: 250 * GB,
            free_bytes: 300 * GB,
        };
        let state = analyze(
            &[
                point(0, 80),
                wrong_capacity,
                impossible,
                point(7, 73),
                point(7, 72),
            ],
            15 * GB,
        );
        assert!(matches!(
            state,
            ForecastState::NeedHistory {
                observations: 2,
                ..
            }
        ));
    }
}
