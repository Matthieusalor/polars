#[cfg(feature = "timezones")]
use chrono_tz::Tz;
use polars_core::prelude::*;
use polars_core::utils::ensure_sorted_arg;
use polars_ops::prelude::*;

use crate::prelude::*;
#[cfg(feature = "timezones")]
use crate::utils::unlocalize_timestamp;

pub trait PolarsUpsample {
    /// Upsample a [`DataFrame`] at a regular frequency.
    ///
    /// # Arguments
    /// * `by` - First group by these columns and then upsample for every group
    /// * `time_column` - Will be used to determine a date_range.
    ///                   Note that this column has to be sorted for the output to make sense.
    /// * `every` - interval will start 'every' duration
    /// * `offset` - change the start of the date_range by this offset.
    ///
    /// The `every` and `offset` arguments are created with
    /// the following string language:
    /// - 1ns   (1 nanosecond)
    /// - 1us   (1 microsecond)
    /// - 1ms   (1 millisecond)
    /// - 1s    (1 second)
    /// - 1m    (1 minute)
    /// - 1h    (1 hour)
    /// - 1d    (1 calendar day)
    /// - 1w    (1 calendar week)
    /// - 1mo   (1 calendar month)
    /// - 1q    (1 calendar quarter)
    /// - 1y    (1 calendar year)
    /// - 1i    (1 index count)
    ///
    /// Or combine them:
    /// "3d12h4m25s" # 3 days, 12 hours, 4 minutes, and 25 seconds
    ///
    /// By "calendar day", we mean the corresponding time on the next
    /// day (which may not be 24 hours, depending on daylight savings).
    /// Similarly for "calendar week", "calendar month", "calendar quarter",
    /// and "calendar year".
    fn upsample<I: IntoVec<String>>(
        &self,
        by: I,
        time_column: &str,
        every: Duration,
        offset: Duration,
    ) -> PolarsResult<DataFrame>;

    /// Upsample a DataFrame at a regular frequency.
    ///
    /// # Arguments
    /// * `by` - First group by these columns and then upsample for every group
    /// * `time_column` - Will be used to determine a date_range.
    ///                   Note that this column has to be sorted for the output to make sense.
    /// * `every` - interval will start 'every' duration
    /// * `offset` - change the start of the date_range by this offset.
    ///
    /// The `every` and `offset` arguments are created with
    /// the following string language:
    /// - 1ns   (1 nanosecond)
    /// - 1us   (1 microsecond)
    /// - 1ms   (1 millisecond)
    /// - 1s    (1 second)
    /// - 1m    (1 minute)
    /// - 1h    (1 hour)
    /// - 1d    (1 calendar day)
    /// - 1w    (1 calendar week)
    /// - 1mo   (1 calendar month)
    /// - 1q    (1 calendar quarter)
    /// - 1y    (1 calendar year)
    /// - 1i    (1 index count)
    ///
    /// Or combine them:
    /// "3d12h4m25s" # 3 days, 12 hours, 4 minutes, and 25 seconds
    ///
    /// By "calendar day", we mean the corresponding time on the next
    /// day (which may not be 24 hours, depending on daylight savings).
    /// Similarly for "calendar week", "calendar month", "calendar quarter",
    /// and "calendar year".
    fn upsample_stable<I: IntoVec<String>>(
        &self,
        by: I,
        time_column: &str,
        every: Duration,
        offset: Duration,
    ) -> PolarsResult<DataFrame>;
}

impl PolarsUpsample for DataFrame {
    fn upsample<I: IntoVec<String>>(
        &self,
        by: I,
        time_column: &str,
        every: Duration,
        offset: Duration,
    ) -> PolarsResult<DataFrame> {
        let by = by.into_vec();
        upsample_impl(self, by, time_column, every, offset, false)
    }

    fn upsample_stable<I: IntoVec<String>>(
        &self,
        by: I,
        time_column: &str,
        every: Duration,
        offset: Duration,
    ) -> PolarsResult<DataFrame> {
        let by = by.into_vec();
        upsample_impl(self, by, time_column, every, offset, true)
    }
}

fn upsample_impl(
    source: &DataFrame,
    by: Vec<String>,
    index_column: &str,
    every: Duration,
    offset: Duration,
    stable: bool,
) -> PolarsResult<DataFrame> {
    let s = source.column(index_column)?;
    ensure_sorted_arg(s, "upsample")?;
    if matches!(s.dtype(), DataType::Date) {
        let mut df = source.clone();
        df.try_apply(index_column, |s| {
            s.cast(&DataType::Datetime(TimeUnit::Milliseconds, None))
        })
        .unwrap();
        let mut out = upsample_impl(&df, by, index_column, every, offset, stable).unwrap();
        out.try_apply(index_column, |s| s.cast(&DataType::Date))
            .unwrap();
        Ok(out)
    } else if by.is_empty() {
        let index_column = source.column(index_column)?;
        upsample_single_impl(source, index_column, every, offset)
    } else {
        let gb = if stable {
            source.group_by_stable(&by)
        } else {
            source.group_by(&by)
        };
        // don't parallelize this, this may SO on large data.
        gb?.apply(|df| {
            let index_column = df.column(index_column)?;
            let mut upsampled_df = upsample_single_impl(&df, index_column, every, offset)?;
            for column in &by {
                 let filled_group = upsampled_df
                     .column(column)?
                     .fill_null(FillNullStrategy::Forward(None))?;
                 upsampled_df.with_column(filled_group)?;
             }
            Ok(upsampled_df)


        })
    }
}

fn upsample_single_impl(
    source: &DataFrame,
    index_column: &Series,
    every: Duration,
    offset: Duration,
) -> PolarsResult<DataFrame> {
    let index_col_name = index_column.name();

    use DataType::*;
    match index_column.dtype() {
        Datetime(tu, tz) => {
            let s = index_column.cast(&Int64).unwrap();
            let ca = s.i64().unwrap();
            let first = ca.into_iter().flatten().next();
            let last = ca.into_iter().flatten().next_back();
            match (first, last) {
                (Some(first), Some(last)) => {
                    let (first, last) = match tz {
                        #[cfg(feature = "timezones")]
                        Some(tz) => (
                            unlocalize_timestamp(first, *tu, tz.parse::<Tz>().unwrap()),
                            unlocalize_timestamp(last, *tu, tz.parse::<Tz>().unwrap()),
                        ),
                        _ => (first, last),
                    };
                    let first = match tu {
                        TimeUnit::Nanoseconds => offset.add_ns(first, None)?,
                        TimeUnit::Microseconds => offset.add_us(first, None)?,
                        TimeUnit::Milliseconds => offset.add_ms(first, None)?,
                    };
                    let range = datetime_range_impl(
                        index_col_name,
                        first,
                        last,
                        every,
                        ClosedWindow::Both,
                        *tu,
                        tz.as_ref(),
                    )?
                    .into_series()
                    .into_frame();
                    range.join(
                        source,
                        &[index_col_name],
                        &[index_col_name],
                        JoinArgs::new(JoinType::Left),
                    )
                },
                _ => polars_bail!(
                    ComputeError: "cannot determine upsample boundaries: all elements are null"
                ),
            }
        },
        dt => polars_bail!(
            ComputeError: "upsample not allowed for index column of dtype {}", dt,
        ),
    }
}

#[cfg(test)]
mod tests {
use polars_core::prelude::*;
use chrono::NaiveDate;
use crate::prelude::*;

    #[test]
    fn test_upsample_groupby() {
        let time = date_range(
            "time",
            NaiveDate::from_ymd_opt(2021, 12, 16)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            NaiveDate::from_ymd_opt(2021, 12, 16)
                .unwrap()
                .and_hms_opt(2, 0, 0)
                .unwrap(),
            Duration::parse("30m"),
            ClosedWindow::Both,
            TimeUnit::Milliseconds,
            None,
        ).unwrap();
        let df = df!(
            "time" => time,
            "groups" => &["a", "a", "b", "b", "b"],
            "values" => &[1.0, 2.0, 3.0, 4.0, 5.0],
        ).unwrap();
        let out1 = df
            .clone()
            .upsample_stable::<[String; 1]>([String::from("groups")], "time", Duration::parse("15m"), Duration::parse("0")).unwrap();
        let df = df!(
            "time" => &[
            NaiveDate::from_ymd_opt(2021, 12, 16).unwrap().and_hms_opt(0, 0, 0).unwrap(),
            NaiveDate::from_ymd_opt(2021, 12, 16).unwrap().and_hms_opt(0, 15, 0).unwrap(),
            NaiveDate::from_ymd_opt(2021, 12, 16).unwrap().and_hms_opt(0, 30, 0).unwrap(),
            NaiveDate::from_ymd_opt(2021, 12, 16).unwrap().and_hms_opt(1, 0, 0).unwrap(),
            NaiveDate::from_ymd_opt(2021, 12, 16).unwrap().and_hms_opt(1, 15, 0).unwrap(),
            NaiveDate::from_ymd_opt(2021, 12, 16).unwrap().and_hms_opt(1, 30, 0).unwrap(),
            NaiveDate::from_ymd_opt(2021, 12, 16).unwrap().and_hms_opt(1, 45, 0).unwrap(),
            NaiveDate::from_ymd_opt(2021, 12, 16).unwrap().and_hms_opt(2, 0, 0).unwrap(),
            ],
            "groups" => &["a", "a", "a", "b", "b", "b", "b", "b"],
            "values" => &[Some(1.0), None, Some(2.0), Some(3.0), None, Some(4.0), None, Some(5.0)],
        ).unwrap();
        assert_eq!(df, out1)

    }
}
