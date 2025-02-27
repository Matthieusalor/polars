use polars_utils::float::IsFloat;

use super::*;

#[inline]
fn ascending_order<T: PartialOrd + IsFloat>(a: &(IdxSize, T), b: &(IdxSize, T)) -> Ordering {
    compare_fn_nan_max(&a.1, &b.1)
}

#[inline]
fn descending_order<T: PartialOrd + IsFloat>(a: &(IdxSize, T), b: &(IdxSize, T)) -> Ordering {
    compare_fn_nan_max(&b.1, &a.1)
}

pub(super) fn arg_sort<I, J, T>(
    name: &str,
    iters: I,
    options: SortOptions,
    null_count: usize,
    len: usize,
) -> IdxCa
where
    I: IntoIterator<Item = J>,
    J: IntoIterator<Item = Option<T>>,
    T: PartialOrd + Send + Sync + IsFloat,
{
    let descending = options.descending;
    let nulls_last = options.nulls_last;

    let mut vals = Vec::with_capacity(len - null_count);

    let null_cap = if nulls_last { null_count } else { len };
    let mut nulls_idx = Vec::with_capacity(null_cap);
    let mut count: IdxSize = 0;

    for arr_iter in iters {
        let iter = arr_iter.into_iter().filter_map(|v| {
            let i = count;
            count += 1;
            match v {
                Some(v) => Some((i, v)),
                None => {
                    // SAFETY: we allocated enough.
                    unsafe { nulls_idx.push_unchecked(i) };
                    None
                },
            }
        });
        vals.extend(iter);
    }

    arg_sort_branch(
        vals.as_mut_slice(),
        descending,
        ascending_order,
        descending_order,
        options.multithreaded,
    );

    let iter = vals.into_iter().map(|(idx, _v)| idx);
    let idx = if nulls_last {
        let mut idx = Vec::with_capacity(len);
        idx.extend(iter);
        if descending {
            idx.extend(nulls_idx.into_iter().rev());
        } else {
            idx.extend(nulls_idx);
        }
        idx
    } else {
        let ptr = nulls_idx.as_ptr() as usize;
        if descending {
            nulls_idx.reverse();
        }
        nulls_idx.extend(iter);
        // We had a realloc.
        debug_assert_eq!(nulls_idx.as_ptr() as usize, ptr);
        nulls_idx
    };

    ChunkedArray::with_chunk(name, IdxArr::from_data_default(Buffer::from(idx), None))
}
